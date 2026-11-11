'use strict';
// Prints, from the markdown-it bundled in upstream Markdoc, the values the unit tests of the
// inline rules text, linkify, newline, escape, backticks, autolink, html_inline, entity,
// balance_pairs, fragments_join (src/markdown_it/rules_inline/) and of common/html_re assert.
// Usage: node inline_rule_values.cjs <@markdoc/markdoc dist/index.js>
const fs = require('fs');
const path = require('path');
const bundlePath = path.resolve(process.argv[2]);
const Markdoc = require(bundlePath);
const MarkdownIt = new Markdoc.Tokenizer().parser.constructor;
const bundle = fs.readFileSync(bundlePath, 'utf8');
// a `var NAME = ...;` line of the bundle, evaluated on its own (module-private regexes)
const bundled = (name) => {
  const a = bundle.indexOf(`var ${name} = `);
  const b = bundle.indexOf(';\n', a) + 2;
  return new Function(bundle.slice(a, b) + `\nreturn ${name};`)();
};
const htmlRe = bundle.slice(bundle.indexOf('var open_tag = '), bundle.indexOf('var HTML_SEQUENCES = '));
const HTML_TAG_RE = new Function(htmlRe + '\nreturn HTML_TAG_RE;')();
const HTML_OPEN_CLOSE_TAG_RE = new Function(htmlRe + '\nreturn HTML_OPEN_CLOSE_TAG_RE;')();
const EMAIL_RE = bundled('EMAIL_RE');
const AUTOLINK_RE = bundled('AUTOLINK_RE');
const hex = (s) => Array.from({ length: s.length }, (_, i) => s.charCodeAt(i).toString(16)).join(' ');
const out = {};
const t = (k, v) => { out[k] = v; };

const md = new MarkdownIt();
const rule = (name) => md.inline.ruler.__rules__.find((r) => r.name === name).fn;
const rule2 = (name) => md.inline.ruler2.__rules__.find((r) => r.name === name).fn;
const state = (src, m = md) => new m.inline.State(src, m, {}, []);
const summary = (s) => s.tokens.map((x) => [x.type, x.content, x.markup, x.info, x.level].join('|'));
// parseInline with only `rules` in the inline ruler and text_join off: children as type|content|markup|info
const only = (rules, opts) => {
  const m = new MarkdownIt(opts || {});
  m.inline.ruler.enableOnly(rules);
  m.core.ruler.disable(['text_join']);
  return (src) => m.parseInline(src, {})[0].children.map((x) => [x.type, x.content, x.markup, x.info].join('|'));
};

// text.rs
t('text_terminators', Array.from({ length: 128 }, (_, c) => c).filter((c) => !rule('text')(state(String.fromCharCode(c)), true)).map((c) => c.toString(16)).join(' '));
{ const s = state('ab\u{1F600}c*d'); const r1 = rule('text')(s, false); const a = [r1, s.pos, s.pending]; const r2 = rule('text')(s, false); s.pos = 6; s.pending = ''; const r3 = rule('text')(s, true); t('text_state', [...a, r2, r3, s.pos, s.pending.length]); }

// newline.rs
{
  const f = only(['text', 'newline']);
  t('newline', ['a\nb', 'a \nb', 'a  \nb', 'a   \n  b', '   \nb', ' \nb', 'a\t\nb', 'a \t \nb', 'a\n \t b', 'a\n\u00A0b'].map((s) => f(s).join(' / ')));
}

// escape.rs
{
  const e = rule('escape');
  t('escaped_table', Array.from({ length: 256 }, (_, c) => { const s = state('\\' + String.fromCharCode(c)); e(s, false); return s.tokens[0].content === String.fromCharCode(c) ? 1 : 0; }).join(''));
  const f = only(['text', 'newline', 'escape']);
  t('escape', ['\\', 'a\\', '\\\n  b', '\\ \nb', 'a\\  \nb', '\\\u{1F600}x', '\\\uD83Dx', '\\\uDE00', '\\\u00E9', '\\\\'].map((s) => f(s).map((x) => x.split('|').map((y) => hex(y)).join('|')).join(' / ')));
}

// backticks.rs
{
  const b = rule('backticks');
  const run = (src, pos, posMax, silent) => { const s = state(src); s.pos = pos; if (posMax !== undefined) s.posMax = posMax; const r = b(s, silent); return [r, s.pos, s.pending, JSON.stringify(s.backticks), summary(s).join(' / ')].join(' ; '); };
  t('backticks', [run('a`b``c`d```e``', 1), run('``a``x', 0, 4), run('``a``x', 0), run('`a``', 0, 3), run('` a `', 0), run('`  `', 0), run('`\na\n`', 0), run('``', 0), run('`x``', 0, undefined, true), run('a`', 1)]);
}

// autolink.rs
t('autolink_re', ['a:b', 'ab:c', 'a'.repeat(32) + ':x', 'a'.repeat(33) + ':x', '1a:x', 'a+b.c-d:x', 'ab:', 'http://a b', 'http://a\u00A0b', 'http://a\u0001', 'http://a\u007F', 'ab:\uD800', 'ab:<'].map((s) => AUTOLINK_RE.test(s)));
t('email_re', ['x@y', 'x@y.z', 'x@-y', 'x@y-', 'x@y..z', 'x@y.', 'x@.y', 'x@' + 'a'.repeat(63), 'x@' + 'a'.repeat(64), 'x@a' + '-'.repeat(61) + 'b', "!#$%&'*+/=?^_`{|}~-@e", '@x', 'x@', 'x@y@z', '\u00E9@x', 'x@\u00E9', 'x@a.b-c.d9'].map((s) => EMAIL_RE.test(s)));

// html_re.rs
const htmlCases = ['<a>', '<a b=c\u00A0d="x">', '<a b=c\u00A0d>', "<a b='>'>", '<a b="x"c>', '<a b=>', '<a / >', '<a/>', '<a b=c/>',
  '</a \n>', '</a b>', '<!-->', '<!--->', '<!---->', '<!-- a -->', '<!-- a --->', '<!-- a ---> b -->', '<!-- a -- b -->',
  '<?>', '<??>', '<? a ?> ?>', '<!DOCTYPE html> x>', '<!1>', '<![CDATA[ ]] ]]> ]]>', '<![CDATA[x', '<a\u0085b>', '<a\uFEFFb>',
  '<a b=c\u3000d=\'>\'>', '<a b=\u00A0c>', '<x-y>', '<1a>', '<a b=c`d>', '<a\n  b\n  c="d"\n>', '<a b="c\nd">'];
t('html_tag_re', htmlCases.map((s) => { const m = s.match(HTML_TAG_RE); return m ? m[0].length : -1; }));
t('html_open_close_tag_re', htmlCases.map((s) => { const m = s.match(HTML_OPEN_CLOSE_TAG_RE); return m ? m[0].length : -1; }));
const lineRe = new RegExp(HTML_OPEN_CLOSE_TAG_RE.source + '\\s*$');
t('html_open_close_tag_line_re', ['<a>', '<a>  \t', '<a> x', '</a>\n', '<a b="x">\u00A0', '<!-- x -->', '<a b=c\u00A0d="x"> '].map((s) => lineRe.test(s)));

// html_inline.rs
{
  const isLinkOpen = (str) => /^<a[>\s]/i.test(str);
  const isLinkClose = (str) => /^<\/a\s*>/i.test(str);
  const cases = ['<a>', '<A>', '<a b>', '<a\u00A0b>', '<a\u0085b>', '<ab>', '</a>', '</A  >', '</a\u3000>', '</ab>', '<a/>'];
  t('link_open_close', cases.map((s) => [isLinkOpen(s), isLinkClose(s)]));
  const m = new MarkdownIt({ html: true });
  const h = m.inline.ruler.__rules__.find((r) => r.name === 'html_inline').fn;
  t('html_inline_level', ['<a href="x">', '</a>', '<A\tb>', '</a >', '<span>', '<a', 'x'].map((src) => { const s = state(src, m); const r = h(s, false); return [r, s.pos, s.linkLevel, summary(s).join(' / ')].join(' ; '); }));
}

// entity.rs
{
  const f = only(['text', 'entity']);
  t('entity', ['&#0;', '&#x10FFFF;', '&#x10FFFD;', '&#9999999;', '&#x1F600;', '&#12345678;', '&#xD800;', '&#X41;', '&NotAnEntity;', '&ngE;', '&amp', '&a;', '&#x;', '&' + 'a'.repeat(32) + ';'].map((s) => f(s).map((x) => x.split('|').map((y) => hex(y)).join('|')).join(' / ')));
}

// balance_pairs.rs
{
  const lp = rule2('balance_pairs');
  const D = (marker, length, token, open, close) => ({ marker, length, token, end: -1, open, close });
  const lists = {
    em: [D(42, 1, 0, true, false), D(42, 1, 2, false, true)],
    strong_em: [D(42, 3, 0, true, false), D(42, 3, 1, true, false), D(42, 3, 2, true, false), D(42, 3, 4, false, true), D(42, 3, 5, false, true), D(42, 3, 6, false, true)],
    rule_of_3: [D(42, 1, 0, true, false), D(42, 2, 2, true, true), D(42, 2, 3, true, true), D(42, 1, 5, false, true)],
    mixed: [D(95, 1, 0, true, false), D(42, 1, 1, true, false), D(95, 1, 3, false, true), D(42, 1, 4, false, true)],
    strike: [D(126, 0, 1, true, false), D(126, 0, 3, false, true), D(126, 0, 5, true, true)],
    unmatched: [D(42, 1, 0, false, true), D(42, 1, 2, true, false), D(42, 2, 4, true, true)],
    jumps: [D(42, 1, 0, true, true), D(95, 1, 2, true, true), D(42, 1, 4, true, true), D(95, 1, 6, true, true), D(42, 1, 8, true, true), D(95, 1, 10, true, true)],
  };
  const res = {};
  for (const [k, l] of Object.entries(lists)) {
    const s = state('');
    s.delimiters = l.map((d) => ({ ...d }));
    s.tokens_meta = [undefined, { delimiters: l.map((d) => ({ ...d, marker: d.marker })) }];
    lp(s);
    res[k] = [s.delimiters, s.tokens_meta[1].delimiters].map((x) => x.map((d) => [d.end, d.open, d.close].join(',')).join(' '));
  }
  t('balance_pairs', res);
  const s = state('');
  s.delimiters = undefined;
  try { lp(s); t('balance_pairs_undefined', 'no throw'); } catch (e) { t('balance_pairs_undefined', e.constructor.name); }
}

// fragments_join.rs
{
  const T = MarkdownIt.Token;
  const tok = (type, nesting, content) => { const x = new T(type, '', nesting); x.content = content; x.level = 7; return x; };
  const s = state('');
  s.tokens = [tok('text', 0, 'a'), tok('text', 0, 'b'), tok('em_open', 1, ''), tok('text', 0, 'c'), tok('text', 0, 'd'), tok('text', 0, 'e'), tok('em_close', -1, ''), tok('text_special', 0, '&'), tok('text', 0, 'f'), tok('s_close', -1, ''), tok('text', 0, 'g'), tok('text', 0, 'h')];
  rule2('fragments_join')(s);
  t('fragments_join', s.tokens.map((x) => [x.type, x.level, x.content].join('|')));
}

// linkify.rs
{
  const m = new MarkdownIt({ linkify: true });
  const l = m.inline.ruler.__rules__.find((r) => r.name === 'linkify').fn;
  t('linkify_early_returns', ['http', 'a:/', ':/', '1://x', '://x'].map((src) => { const s = state(src, m); s.pos = src.indexOf(':') < 0 ? 0 : src.indexOf(':'); s.pending = src.slice(0, s.pos); return l(s, false); }));
}

console.log(JSON.stringify(out, null, 1));
