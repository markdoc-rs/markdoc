'use strict';
// How deep upstream's tokenizer (maxNesting 1e9) nests each construct before V8 throws
// RangeError: Maximum call stack size exceeded.
//   node node_depth.cjs DIST probe CONSTRUCT N   -> "ok" or "RangeError" (one tokenize, in
//                                                  this fresh process: cold V8)
//   node node_depth.cjs DIST warm                -> the deepest N per construct after 3,000
//                                                  warm-up documents (binary search in one
//                                                  process: hot V8)
const path = require('path');
const Markdoc = require(path.resolve(process.argv[2]));
const t = new Markdoc.Tokenizer({ allowIndentation: true, allowComments: true, maxNesting: 1e9 });
const nested = {
  'blockquote': (n) => '> '.repeat(n) + 'x\n',
  'list': (n) => '- '.repeat(n) + 'x\n',
  'ordered': (n) => '1. '.repeat(n) + 'x\n',
  'quoted-list': (n) => '> - '.repeat(n) + 'x\n',
  'image': (n) => '!['.repeat(n) + 'x' + '](u)'.repeat(n) + '\n',
  'link-image': (n) => '[!['.repeat(n) + 'x' + '](u)](v)'.repeat(n) + '\n',
  'bracket': (n) => '['.repeat(n) + 'x\n',
};
const fits = (name, n) => {
  try { t.tokenize(nested[name](n)); return true; } catch (e) { if (e instanceof RangeError) return false; throw e; }
};
if (process.argv[3] === 'probe') {
  console.log(fits(process.argv[4], Number(process.argv[5])) ? 'ok' : 'RangeError');
} else {
  for (let i = 0; i < 3000; i++) t.tokenize('> - a *b* [c](d) ![e](f)\n> 1. [[g]] {% x %}\n\n- > h\n');
  for (const name of Object.keys(nested)) {
    let lo = 1, hi = 40000;
    while (hi - lo > 1) { const mid = (lo + hi) >> 1; if (fits(name, mid)) lo = mid; else hi = mid; }
    console.log(name, lo);
  }
}
