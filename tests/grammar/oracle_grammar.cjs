'use strict';
// The JavaScript half of the tag grammar's verification harness: upstream Markdoc 0.5.11's
// generated tag parser (src/grammar/tag.js), called as Markdoc calls it, with the Ast.Variable
// and Ast.Function classes of the published package.
//
//   node oracle_grammar.cjs TAG_JS MARKDOC_DIST < requests.jsonl > results.jsonl
//
//   TAG_JS        upstream's src/grammar/tag.js (v0.5.11)
//   MARKDOC_DIST  @markdoc/markdoc 0.5.11 dist/index.js
//   request       {id, content}
//   output        {id, result}, or {id, error: {message, expected, found, location}} where the
//                 parser throws its SyntaxError ({id, crash} for any other exception)
//
// Each output line is JSON.stringify's text; examples/grammar_parse.rs prints the same for
// the Rust port. Run both through compare.py.
const path = require('path');
const readline = require('readline');

const tag = require(path.resolve(process.argv[2]));
const Markdoc = require(path.resolve(process.argv[3]));
const options = { Variable: Markdoc.Ast.Variable, Function: Markdoc.Ast.Function };

function answer(id, content) {
  try {
    return JSON.stringify({ id, result: tag.parse(content, options) });
  } catch (e) {
    if (!(e instanceof tag.SyntaxError)) return JSON.stringify({ id, crash: String(e) });
    const { message, expected, found, location } = e;
    return JSON.stringify({ id, error: { message, expected, found, location } });
  }
}

let buffer = [];
const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on('line', (line) => {
  if (!line.trim()) return;
  const { id, content } = JSON.parse(line);
  buffer.push(answer(id, content));
  if (buffer.length >= 4096) {
    process.stdout.write(buffer.join('\n') + '\n');
    buffer = [];
  }
});
rl.on('close', () => {
  if (buffer.length) process.stdout.write(buffer.join('\n') + '\n');
});
