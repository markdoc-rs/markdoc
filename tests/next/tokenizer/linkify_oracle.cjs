'use strict';
// linkify-it 6.1.0 as upstream Markdoc 0.5.11 bundles it (the `md.linkify` of its
// markdown-it), answering for each text what the port's LinkifyIt must answer.
//
//   node linkify_oracle.cjs DIST/index.js < requests.jsonl > answers.jsonl
//
// A request is {id, text, options?} (options: linkify-it's own, e.g. {fuzzyLink: true}); the
// answer is {id, test, match, matchAtStart}: match a list of {schema, index, lastIndex, raw,
// text, url} or null, matchAtStart one of those or null.
const path = require('path');
const readline = require('readline');
const Markdoc = require(path.resolve(process.argv[2]));

const MarkdownIt = new Markdoc.Tokenizer().parser.constructor;
const LinkifyIt = new MarkdownIt({ linkify: true }).linkify.constructor;
const cache = new Map();
function linkify(options) {
  const key = JSON.stringify(options || {});
  if (!cache.has(key)) cache.set(key, new LinkifyIt(options || {}));
  return cache.get(key);
}
const plain = (m) => m && { schema: m.schema, index: m.index, lastIndex: m.lastIndex, raw: m.raw, text: m.text, url: m.url };

readline.createInterface({ input: process.stdin, crlfDelay: Infinity }).on('line', (line) => {
  if (!line.trim()) return;
  const { id, text, options } = JSON.parse(line);
  const l = linkify(options);
  const answer = { id };
  try {
    answer.test = l.test(text);
    const m = l.match(text);
    answer.match = m && m.map(plain);
    answer.matchAtStart = plain(l.matchAtStart(text));
  } catch (e) {
    answer.threw = { name: e.name, message: e.message };
  }
  process.stdout.write(JSON.stringify(answer) + '\n');
});
