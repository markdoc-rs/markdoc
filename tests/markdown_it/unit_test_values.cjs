'use strict';
// Prints, from node and from the markdown-it bundled in upstream Markdoc, the values the unit
// tests under src/markdown_it/ assert (js, token, ruler, mod, common/utils, helpers,
// state_block, state_inline): where those expectations came from.
// Usage: node unit_test_values.cjs <@markdoc/markdoc dist/index.js>
const fs = require('fs');
const path = require('path');
const bundlePath = path.resolve(process.argv[2]);
const Markdoc = require(bundlePath);
const MarkdownIt = new Markdoc.Tokenizer().parser.constructor;
const md = new MarkdownIt();
const U = md.utils;
// decodeHTMLStrict exactly as bundled (not exported): evaluate the bundle's entities section on its own
const src = fs.readFileSync(bundlePath, 'utf8');
const a = src.indexOf('// node_modules/entities/dist/decode-codepoint.js');
const b = src.indexOf('// node_modules/entities/dist/index.js');
const decodeHTMLStrict = new Function(src.slice(a, b) + '\nreturn decodeHTMLStrict;')();
const hex = (s) => Array.from({ length: s.length }, (_, i) => s.charCodeAt(i).toString(16));
const out = {};
const t = (k, v) => { out[k] = v; };
const s = 'a\u{1F600}';
t('charCodeAt', [s.charCodeAt(1), s.charCodeAt(9), s.codePointAt(1), s.codePointAt(2), s.codePointAt(3), '\uD83D'.codePointAt(0)]);
t('fromCharCode', hex(String.fromCharCode(0x10041, 0xD800)));
t('fromCodePoint', hex(String.fromCodePoint(0x1F600, 0xDC00)));
try { String.fromCodePoint(0x110000); t('fcp_throw', false); } catch (e) { t('fcp_throw', e.constructor.name); }
const x = 'abcdef';
t('slice', [x.slice(-2), x.slice(1, -1), x.slice(4, 2), x.slice(2, 99), x.slice(-99, 2), x.slice(3, 99)]);
t('substring', [x.substring(4, 1), x.substring(-3, 2)]);
t('substr', [x.substr(-3, 2), x.substr(2), x.substr(1, -1)]);
const y = 'abcabc';
t('indexOf', [y.indexOf('c', 3), y.indexOf('', 99), y.indexOf('x'), y.indexOf('abcabcd')]);
t('lastIndexOf', [y.lastIndexOf('a'), y.lastIndexOf('a', 2), y.lastIndexOf('bc', 99), y.lastIndexOf('')]);
t('startsEnds', ['abc'.startsWith('bc', 1), 'abc'.startsWith('a', 9), 'abc'.startsWith('', 9), 'abc'.endsWith('ab', 2), 'abc'.endsWith('abcd'), 'abc'.includes('c', 2)]);
t('split', ['a\nb\n'.split('\n'), ''.split('\n'), 'ab'.split(''), ''.split(''), 'aXXb'.split('XX'), 'XX'.split('XX')]);
t('trim', ['﻿ a b　\n'.trim(), '  a '.trimStart(), '  a '.trimEnd(), ' \t '.trim(), '᠎'.trim().length, '\u0085'.trim().length]);
t('case', [hex('ẞ'.toLowerCase()), hex('ß'.toUpperCase()), hex('İ'.toLowerCase()), hex('ΑΣ'.toLowerCase()), hex('ΑΣ Α'.toLowerCase()), hex('Σ'.toLowerCase()), hex('ﬀ'.toUpperCase()), hex('ŉ'.toUpperCase()), hex('ÁΣ'.toLowerCase()), hex('AΣ́b'.toLowerCase()), hex('A\uD800Σ'.toLowerCase()), hex('AΣ\uDC00'.toLowerCase()), hex('a\uDBFFb'.toUpperCase()), hex('\u{10428}'.toUpperCase())]);
t('numstr', [0, -0, 1, -1.5, 123456789, 0.1, 1e21, 1.5e21, 1e20, 1e-6, 1e-7, 1.25e-7, Infinity, NaN, -Infinity, 2 ** 53].map(String));
t('parseInt', [parseInt('ff', 16), parseInt('0x1F', 16), parseInt('0x1F', 10), parseInt('  -12abc', 10), parseInt('FFFFFFFF', 16), parseInt('99999999', 10), parseInt('g', 16), parseInt('', 10), parseInt('1', 37), parseInt('z', 36)].map(String));
t('Number', ['', '  ', '007', '123456789', ' 1.5 ', '5.', '.5', '-2e3', '0x10', '0b101', '0o17', 'Infinity', '.', 'e5', '1e', '0x', '-0x10', '1_0', 'infinity', '1 2', '+-1'].map((v) => String(Number(v))));
t('arrayReplaceAt', [U.arrayReplaceAt([1, 2, 3], 1, [7, 8]), U.arrayReplaceAt([1, 2, 3], 5, [7, 8]), U.arrayReplaceAt([1, 2, 3], 2, [7, 8])]);
t('isValidEntityCode', [0x41, 0xD800, 0xFDD0, 0x1FFFF, 0x10FFFE, 0x08, 0x09, 0x0B, 0x0C, 0x1F, 0x7F, 0x9F, 0xA0, 0x10FFFD, 0x110000].map(U.isValidEntityCode));
t('utilsFromCodePoint', [hex(U.fromCodePoint(0x1F600)), hex(U.fromCodePoint(0x41)), hex(U.fromCodePoint(0x110000))]);
t('unescapeMd', [U.unescapeMd('\\*a\\b\\\\'), U.unescapeMd('plain')]);
t('unescapeAll', ['a\\*b', '&amp;&lt;', '&ngE;', '&#x41;&#66;', '&#X41;', '&#0;', '&#xD800;', '&#123456789;', '&#x110000;', '&#1a;', '&amp', '&ampx;', '&AMP;', '&Amp;', '&#x;', '&#xg;', '&#abc;', '\\&amp;', '&#128;', '&#9;', '&#x1F600;', '&aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;'].map((v) => hex(U.unescapeAll(v)).join(' ')));
t('decodeHTMLStrict', ['&amp;', '&amp', 'a&lt;b&gt;c', '&&amp;', '&#128;', '&#0;', '&#xD800;', '&#99999999999999999999999;', '&#65', '&#x;', '&NotNestedGreaterGreater;', '&nosuch;'].map((v) => hex(decodeHTMLStrict(v)).join(' ')));
t('isWhiteSpace', [0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x20, 0xA0, 0x1680, 0x2000, 0x200A, 0x202F, 0x205F, 0x3000, 0x2028, 0x2029, 0xFEFF, 0x85, 0x180E, 0x200B].map(U.isWhiteSpace));
t('isSpace', [0x20, 0x09, 0x0A, 0xA0].map(U.isSpace));
t('mdAsciiPunct', Array.from({ length: 128 }, (_, c) => c).filter(U.isMdAsciiPunct).map((c) => String.fromCharCode(c)).join(''));
t('isPunctCharCode', [0x21, 0x24, 0x41, 0xA7, 0x2019, 0x20AC, 0x1F600, 0x10100, 0xD800, 0x3000, 0x110000].map(U.isPunctCharCode));
t('isPunctChar', [U.isPunctChar('a!'), U.isPunctChar('\uD83D'), U.isPunctChar('abc')]);
t('normalizeReference', ['  Foo \t\n bar ', 'ẞ', 'Θϴθϑ', 'a 　b', 'İ', 'Ω'].map((v) => hex(U.normalizeReference(v)).join(' ')));
t('asciiTrim', [U.asciiTrim(' \t\n\r a b  \r\n'), U.asciiTrim('  ')]);
t('escapeHtml', U.escapeHtml('<a href="x">&\'</a>'));
t('escapeRE', U.escapeRE('a.b*c(d)|[e]-{f}^$?+\\'));
const T = MarkdownIt.Token; const tk = new T('link_open', 'a', 1);
tk.attrSet('href', '/x'); tk.attrPush(['title', 'T']); const ti = tk.attrIndex('title'); tk.attrSet('href', '/y');
tk.attrJoin('class', 'a'); tk.attrJoin('class', 2); tk.attrSet('start', 3); tk.attrJoin('start', 1.5);
t('token', [ti, tk.attrGet('href'), tk.attrGet('class'), tk.attrGet('start'), tk.attrs.length, tk.attrIndex('nope'), tk.attrGet('nope')]);
t('validateLink', ['http://x', 'javascript:alert(1)', '  JavaScript:x', 'vbscript:x', 'file:///etc', 'data:text/html,x', 'data:image/png;base64,x', 'DATA:IMAGE/WEBP;x', 'data:image/svg+xml;x', 'javascript'].map((v) => md.validateLink(v)));
t('mdurlFormat', [U.lib.mdurl.format({ protocol: 'http:', slashes: true, auth: 'u:p', hostname: '::1', port: '8080', pathname: '/a', search: '?q', hash: '#h' }), U.lib.mdurl.format({ auth: '', port: '', hostname: 'h' })]);
let errs = [];
const m2 = new MarkdownIt();
try { m2.disable(['code', 'nope', 'nada']); } catch (e) { errs.push(e.message); }
try { m2.enable(['nope']); } catch (e) { errs.push(e.message); }
try { new MarkdownIt('nope'); } catch (e) { errs.push(e.message); }
t('errors', errs);
const cm = new MarkdownIt('commonmark');
t('commonmark', [cm.options.html, cm.options.xhtmlOut, cm.options.maxNesting]);
t('zero', new MarkdownIt('zero').block.ruler.__rules__.filter((r) => r.enabled).map((r) => r.name));
// js.rs case_mapping_of_every_code_point_matches_node: FNV-1a (32-bit) over the UTF-16 units
// (then the length) of toLowerCase/toUpperCase of every code point (surrogates as lone
// units), and of toLowerCase in three Final_Sigma contexts
{
  let h1 = 0x811c9dc5, h2 = 0x811c9dc5;
  const add = (h, s) => {
    for (let i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 0x01000193) >>> 0; }
    h ^= s.length; return Math.imul(h, 0x01000193) >>> 0;
  };
  for (let c = 0; c <= 0x10ffff; c++) {
    const s = c >= 0xd800 && c <= 0xdfff ? String.fromCharCode(c) : String.fromCodePoint(c);
    h1 = add(h1, s.toLowerCase()); h1 = add(h1, s.toUpperCase());
    h2 = add(h2, ('A' + s + '\u03A3').toLowerCase());
    h2 = add(h2, ('A\u03A3' + s + 'A').toLowerCase());
    h2 = add(h2, (s + '\u03A3').toLowerCase());
  }
  t('caseHash', ['0x' + h1.toString(16), '0x' + h2.toString(16)]);
}
process.stdout.write(JSON.stringify(out, null, 0).replace(/\],"/g, '],\n"').replace(/,"(?=[a-zA-Z]+":)/g, ',\n"') + '\n');

// helpers/mod.rs tests
{
const md = new MarkdownIt();
// every inline rule off, so skipToken only steps one code unit (as with the Rust stubs)
md.inline.ruler.enableOnly([]);
const H = md.helpers;
const label = (src, start, dn, posMax) => {
  const st = new md.inline.State(src, md, {}, []);
  if (posMax !== undefined) st.posMax = posMax;
  st.pos = 0;
  const r = H.parseLinkLabel(st, start, dn);
  return [r, st.pos];
};
console.log('label', JSON.stringify([label('[foo]', 0), label('[a[b]c] x', 0), label('[a[b]c', 0), label('[a\\]b]', 0), label('[a[b]c]', 0, true), label('[foo]', 0, false, 3), label('x[y]', 1), label('[]', 0)]));
const dests = [['<foo bar>', 0, 9], ['<foo\nbar>', 0, 9], ['<a<b>', 0, 5], ['<a\\>b>', 0, 6], ['foo(bar)baz qux', 0, 15], ['foo(bar', 0, 7], ['a\\ b c', 0, 6], ['', 0, 0], ['('.repeat(33) + ')'.repeat(33), 0, 66], ['('.repeat(32) + ')'.repeat(32), 0, 64], ['&amp;x', 0, 6], ['a\x01b', 0, 3], ['<>', 0, 2], ['<a', 0, 2], ['abc def', 0, 2], ['a)b', 0, 3], ['<a\\', 0, 3], ['x<y', 1, 3]];
console.log('dest', JSON.stringify(dests.map(([s, a, b]) => H.parseLinkDestination(s, a, b))));
const titles = [['"title" x', 0, 9], ["'t\\'x'", 0, 6], ['(a(b)', 0, 5], ['"abc', 0, 4], ['x', 0, 1], ['', 0, 0], ['"a\\"b"', 0, 6], ['(a)', 0, 3], ['"&amp;"', 0, 7]];
console.log('title', JSON.stringify(titles.map(([s, a, b]) => H.parseLinkTitle(s, a, b))));
const prev = H.parseLinkTitle('"ab\n', 0, 4);
console.log('cont', JSON.stringify([prev, H.parseLinkTitle('cd" x', 0, 5, prev), H.parseLinkTitle('(x', 0, 2, H.parseLinkTitle('(a\n', 0, 3))]));
}

// state_block.rs and state_inline.rs tests
{
const md = new MarkdownIt();
const blk = (src) => {
  const s = new md.block.State(src, md, {}, []);
  return { b: s.bMarks, e: s.eMarks, t: s.tShift, s: s.sCount, bs: s.bsCount, lineMax: s.lineMax };
};
console.log('block', JSON.stringify([blk(''), blk('a'), blk('a\n'), blk('  a\n\tb\n \t c'), blk('x\n   '), blk('\n\n'), blk('> q\n')]));
const s = new md.block.State('  ab\n\t\tcd\n   \n>  ef\n', md, {}, []);
const gl = [[0, 2, 0, false], [0, 2, 2, false], [1, 2, 2, true], [1, 2, 6, false], [0, 4, 1, true], [3, 3, 0, false], [2, 4, 3, false], [0, 9, 0, false], [0, 9, 0, true]];
console.log('getLines', JSON.stringify(gl.map((a) => s.getLines(...a))));
// tShift masking: pretend line 3's '>' is part of the indent (as blockquote does)
s.tShift[3] = 1; s.bsCount[1] = 1;
console.log('getLinesMasked', JSON.stringify([s.getLines(3, 4, 3, false), s.getLines(1, 2, 3, false)]));
console.log('skips', JSON.stringify([s.skipSpaces(0), s.skipSpaces(2), s.skipSpacesBack(2, 0), s.skipSpacesBack(4, 2), s.skipSpacesBack(0, 0), s.skipChars(5, 9), s.skipCharsBack(7, 9, 5), s.skipCharsBack(5, 9, 5), s.isEmpty(2), s.isEmpty(0), s.isEmpty(99), s.skipEmptyLines(2), s.skipEmptyLines(4)]));
const p = s.push('x_open', 'x', 1); const q = s.push('y', '', 0); const r = s.push('x_close', 'x', -1);
console.log('push', JSON.stringify([p.level, p.block, q.level, r.level, s.level]));
// inline
const scan = (src, start, split, posMax) => { const st = new md.inline.State(src, md, {}, []); if (posMax !== undefined) st.posMax = posMax; return st.scanDelims(start, split); };
console.log('scan', JSON.stringify([scan('*a*', 0, true), scan('*a*', 2, true), scan('a**b', 1, false), scan(' _a', 1, false), scan('a_', 1, false), scan('\u{1F600}*x', 2, true), scan('x*\u{1F600}', 1, true), scan('\uDE00*x', 1, true), scan('a\uDE00*x', 2, true), scan('x*\uD83D', 1, true), scan('x*\uDE00', 1, true), scan('!**a', 1, false), scan('a**!', 1, false), scan('***', 0, true, 2), scan(' *a', 1, true), scan('€*a', 1, false), scan('a*€', 1, false)]));
const st = new md.inline.State('abc', md, {}, []);
st.pending = 'ab';
const t1 = st.push('link_open', 'a', 1);
st.delimiters.push({ marker: 42, length: 1, token: 0, end: -1, open: true, close: false });
st.pending = 'c';
const t2 = st.push('link_close', 'a', -1);
const t3 = st.push('x_close', '', -1);
const undef = st.delimiters === undefined;
st.delimiters = [];
const t4 = st.push('y_open', '', 1);
console.log('ipush', JSON.stringify([st.tokens.map((t) => [t.type, t.level, t.content]), st.tokens_meta.map((m) => (m ? m.delimiters.length : null)), undef, st.level, st.pendingLevel, st._prev_delimiters.length]));
}

// ruler.rs tests (`Ruler` is the bundle's class)
{
const Ruler = MarkdownIt.Ruler;
const run = (r, c) => r.getRules(c).map((f) => f());
const one = () => 1, two = () => 2, three = () => 3;
let r = new Ruler();
r.push('a', one, { alt: ['x'] }); r.push('c', three); r.before('c', 'b', two, { alt: ['x', 'y'] });
const out = [run(r, ''), run(r, 'x'), run(r, 'y'), run(r, 'nope')];
r.after('a', 'a2', three, { alt: ['y'] }); out.push(r.__rules__.map((x) => x.name), run(r, 'y'));
r.at('b', one); out.push(run(r, ''), run(r, 'x'));
try { r.at('zz', one); } catch (e) { out.push(e.message); }
r = new Ruler(); r.push('a', one, { alt: ['x'] }); r.push('b', two, { alt: ['x'] }); r.push('c', three);
out.push(r.disable(['a', 'zz'], true), run(r, ''), run(r, 'x'));
r.disable(['b']); out.push(run(r, 'x')); out.push(r.enable(['b'])); out.push(run(r, 'x'));
r.enableOnly(['c']); out.push(run(r, ''));
try { r.disable(['c', 'zz']); } catch (e) { out.push(e.message); }
r = new Ruler(); r.push('a', one); r.push('b', two); r.disable(['a', 'b']); out.push(run(r, ''));
try { r.enable(['a', 'zz', 'b']); } catch (e) { out.push(e.message); }
out.push([r.__rules__[0].enabled, r.__rules__[1].enabled], run(r, ''));
r.push('c', three); out.push(run(r, ''));
console.log(JSON.stringify(out));
}
