'use strict';
// Upstream's token stream for each case, for tests/next/tokenizer/golden.py.
//
//   node oracle_tokens.cjs DIST/index.js < cases.jsonl > answers.jsonl
//
// A case is {id, code, tokenizer} (tokenizer: the Tokenizer's config, as given); the answer
// is {id, tokens} (the tokens as JSON.stringify writes them, as tests/next/oracle.cjs writes
// them) or {id, threw: {name, message}} where tokenize throws.
//
// With `lossy: true` in a case, the inline tokens' children have each lone surrogate in their
// content replaced by U+FFFD just before smartquotes runs: what the port's tokens hold from
// the start (a Rust string cannot hold one), for gate.py to tell that divergence (a lone
// surrogate read back from a token, see src/markdown_it/mod.rs) from any other difference.
const path = require('path');
const readline = require('readline');
const Markdoc = require(path.resolve(process.argv[2]));

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on('line', (line) => {
  if (!line.trim()) return;
  const { id, code, tokenizer, lossy } = JSON.parse(line);
  let answer;
  try {
    const t = new Markdoc.Tokenizer(tokenizer || {});
    if (lossy) {
      t.parser.core.ruler.before('smartquotes', 'lossy', (state) => {
        for (const block of state.tokens) {
          for (const child of block.children || []) child.content = child.content.toWellFormed();
        }
      });
    }
    const tokens = t.tokenize(code);
    answer = { id, tokens: JSON.parse(JSON.stringify(tokens)) };
  } catch (e) {
    answer = { id, threw: { name: e.name, message: e.message } };
  }
  process.stdout.write(JSON.stringify(answer) + '\n');
});
