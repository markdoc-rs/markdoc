'use strict';
// Checks that the tag grammar bundled in upstream's published package (dist/index.js) behaves
// as src/grammar/tag.js, the file the Rust port follows: for each request {id, content}, the
// dist's Tokenizer tokenizes `{% content %}`, and the one token Markdoc's annotations plugin
// makes from it must carry what tag.js gives for the content the plugin hands its parser
// (the content after markdown-it's normalization, trimmed):
//   - a result: the token's type, nesting (0 when absent), info (the content) and meta
//     (compared as JSON);
//   - a SyntaxError: an `error` token whose meta.error.message is the error's message, and
//     whose location is the error's offsets plus the content's start (2) when the block rule
//     made the token, or null when the inline rule did.
// Requests whose `{% %}` would not be one tag (a `%}` outside a string in the content, an
// unclosed string) are not tokenized; documents in which Markdown splits the tag into other
// blocks (the inline rule handles `$...`, and a paragraph ends at a blank line or before a
// heading, list, quote or fence) are counted as "split", not compared.
//
//   node dist_check.cjs TAG_JS MARKDOC_DIST < requests.jsonl
//
// Prints a tally; exits 1 on any disagreement.
const path = require('path');
const readline = require('readline');

const tag = require(path.resolve(process.argv[2]));
const Markdoc = require(path.resolve(process.argv[3]));
const options = { Variable: Markdoc.Ast.Variable, Function: Markdoc.Ast.Function };
const tokenizer = new Markdoc.Tokenizer();
const TAG_TYPES = new Set(['tag_open', 'tag_close', 'tag', 'annotation', 'variable', 'error']);

// findTagEnd from upstream src/utils.ts
function findTagEnd(content, start = 0) {
  let state = 0;
  for (let pos = start; pos < content.length; pos++) {
    const char = content[pos];
    switch (state) {
      case 1:
        if (char === '"') state = 0;
        else if (char === '\\') state = 2;
        break;
      case 2:
        state = 1;
        break;
      case 0:
        if (char === '"') state = 1;
        else if (content.startsWith('%}', pos)) return pos;
    }
  }
  return null;
}

function flatten(tokens, out = []) {
  for (const t of tokens) {
    out.push(t);
    if (Array.isArray(t.children)) flatten(t.children, out);
  }
  return out;
}

const tally = { requests: 0, notOneTag: 0, split: 0, results: 0, errors: 0, disagree: 0 };
const shown = [];

function check(id, content) {
  tally.requests++;
  // markdown-it's normalize rule: \r\n and \r become \n, \0 becomes U+FFFD
  const normalized = content.replace(/\r\n?|\n/g, '\n').replace(/\0/g, '�');
  const src = `{% ${normalized} %}`;
  if (findTagEnd(src, 0) !== src.length - 2) {
    tally.notOneTag++;
    return;
  }
  const inner = normalized.trim();
  let expected, error;
  try {
    expected = tag.parse(inner, options);
  } catch (e) {
    if (!(e instanceof tag.SyntaxError)) throw e;
    error = e;
  }
  const found = flatten(tokenizer.tokenize(src)).filter((t) => TAG_TYPES.has(t.type));
  if (found.length !== 1) {
    tally.split++;
    return;
  }
  const t = found[0];
  let ok;
  if (error) {
    tally.errors++;
    const location = t.map
      ? { start: { offset: error.location.start.offset + 2 }, end: { offset: error.location.end.offset + 2 } }
      : null;
    ok = t.type === 'error' &&
      t.meta.error.message === error.message &&
      JSON.stringify(t.meta.error.location) === JSON.stringify(location);
  } else {
    tally.results++;
    ok = t.type === expected.type &&
      t.nesting === (expected.nesting ?? 0) &&
      t.info === inner &&
      JSON.stringify(t.meta) === JSON.stringify(expected.meta);
  }
  if (!ok) {
    tally.disagree++;
    if (shown.length < 10) shown.push({ id, content, token: { type: t.type, nesting: t.nesting, info: t.info, meta: t.meta }, expected: expected ?? { message: error.message, location: error.location } });
  }
}

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on('line', (line) => {
  if (!line.trim()) return;
  const { id, content } = JSON.parse(line);
  check(id, content);
});
rl.on('close', () => {
  for (const s of shown) console.log(JSON.stringify(s));
  console.log(`requests ${tally.requests}  compared ${tally.results + tally.errors} (results ${tally.results}, errors ${tally.errors})  split by Markdown ${tally.split}  not one tag ${tally.notOneTag}  disagree ${tally.disagree}`);
  process.exitCode = tally.disagree ? 1 : 0;
});
