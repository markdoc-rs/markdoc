// Token streams for the parser port's unit tests (src/next/parser/tests.rs), made by
// upstream Markdoc 0.5.11's own tokenizer, because the tokenizer port lives on another
// branch until the integration.
//
// Usage: node src/next/parser/fixtures.cjs <@markdoc/markdoc 0.5.11's dist/index.js> \
//          > src/next/parser/fixtures.json
//
// Each case is {src, tokens}: `tokens` are upstream's tokens of `src` as JSON.stringify writes
// them (or of `tokenize`, when the test parses tokens of one text against another as its
// source; or the case's own hand-made tokens, as they are).
// `convert: true` is parser.test.ts's `convert()`: `example.replace(/\n\s+/gm, '\n').trim()`
// tokenized with `new Tokenizer({ allowComments: true })`. Otherwise the tokenizer options
// are `options` (default: upstream's `new Tokenizer()`, both switches off).
'use strict';
const path = require('path');
const Markdoc = require(path.resolve(process.argv[2]));

const fence = '```';
const table = (rows) => `
        {% table %}
        - column 1
        - column 2
        ---
        ${rows.join('\n')}
        {% /table %}
      `;

// parser.test.ts's inputs, by the name of the test that uses them.
const upstream = {
  'heading': `# This is a test`,
  'simple frontmatter': `
      ---
      foo: bar
      ---
      `,
  'frontmatter followed by content': `
      ---
      foo: bar
      test: true
      ---

      # This is a test
      `,
  'emphasis a*b*c': `a*b*c`,
  'emphasis a**b**c': `a**b**c`,
  'emphasis _foo_ bar': `_foo_ bar`,
  'emphasis __foo__ bar': `__foo__ bar`,
  'emphasis foo *bar* baz': `foo *bar* baz`,
  'emphasis foo **bar** baz': `foo **bar** baz`,
  'sample heading': `# Sample Heading`,
  'unordered list': `
      * Example 1
      * Example 2
      * Example 3
      `,
  'ordered list': `
      1. Example 1
      2. Example 2
      3. Example 3
      `,
  'numbered list from 3': `
      3. Example 1
      4. Example 2
      5. Example 3
      `,
  'numbered list from 1': `
      1. Example 1
      4. Example 2
      5. Example 3
      `,
  'link with one word': `
      [foo](/bar)
      `,
  'link with one word and a title': `
      [foo](/bar "title")
      `,
  'link': `
      [this is a test](/bar)
      `,
  'text': `
      This is a test
      `,
  'fence simple': `
      ${fence}ruby
      This is a test
      ${fence}
      `,
  'fence complex': `
      ${fence}ruby this is a test
      This is a test
      ${fence}
      `,
  'fence empty': `
      ${fence}
      This is a test
      ${fence}
      `,
  'image with no title': `![foo](/url)`,
  'image with a title': `![foo](/url "title")`,
  'table with alignments': `
      | Left | Center | Right |
      | :--- | :----: | ----: |
      | Left | Center | Right |
      `,
  'handling a header': `
    # Sample Heading
    
    This is a sample paragraph
    `,
  'handling an image': '![Alt](/logo.png)',
  'lists with bullets': `
      * foo
      * bar
      `,
  'lists with numbers': `
      1. foo
      2. bar
      `,
  'fenced code with a language': `
      ${fence}ruby
      puts "foo"
      ${fence}
      `,
  'fenced code with an annotation': `
      ${fence}ruby {% #foo .bar %}
      test
      ${fence}
      `,
  'block tag with a class': `
        {% callout .foo .bar %}
        ### Heading

        This is a paragraph
        {% /callout %}
        `,
  'block tags nested': `
        {% callout %}
        {% callout %}
        This is a test
        {% /callout %}
        {% /callout %}
        `,
  'annotation id in a header': `# Sample Heading {% #foo-bar %}`,
  'annotation class in a header': `# Sample Heading {% .foo-bar .test %}`,
  'annotation complex values in a header': `# Sample Heading {% #asdf .foo-bar .test foo="bar" %}`,
  'variable by itself': `{% $test %}`,
  'variable in text': `This is a test: {% $test %}`,
  'variable with nested property access': '{% $bar.baz[1].test %}',
  'nested tags with indentation': `
{% tag1 %}
      {% tag2 %}
          Contents
      {% /tag2 %}
{% /tag1 %}
    `,
  'comments': `
    this is a test

    <!-- foo -->
    `,
  'duplicate attributes': `{% foo bar=1 bar=2 bar=3 bar=4 /%}`,
  'duplicate ids': `{% foo #bar #baz #qux /%}`,
  'duplicate annotation values': `testing {% foo=1 foo=2 %}`,
  'duplicates across annotations': `testing {% foo=1 %} another test {% foo=1 %}`,
  'multiple classes': `{% foo .bar .baz .qux /%}`,
  'annotations in a fence': `
    ~~~
    test
    {% #foo %}
    test
    ~~~
    `,
  'identifies inlines': `
    # This is a test

    {% foo %}
    Another {% bar %}test{% /bar %} test
    {% /foo %}

    * bar
    `,
  'unmatched closing tag': `
    {% foo %}
    Test
    {% /bar %}
    `,
  'missing opening': `
      This a test
      {% /foo %}
      `,
  'missing closing tag': `
    {% foo %}
    Test
    `,
  'table keeps if': table([
    `
        {% if $fakeCondition.condition1 %}
        - cell 1
        - cell 2
        {% else /%}
        - cell 3
        - cell 4
        {% /if %}
        `,
  ]),
  'table drops unregistered': table([
    `
        {% if-pref conditions=[{platform: 'web'}] %}
        - cell 1
        - cell 2
        {% /if-pref %}
        {% if-pref conditions=[{platform: 'ios'}] %}
        - cell 1
        - cell 2
        {% /if-pref %}
        `,
  ]),
  'table registered tags': table([
    `
        {% if-pref conditions=[{platform: 'web'}] %}
        - cell 1
        - cell 2
        {% /if-pref %}
        {% if-pref conditions=[{platform: 'ios'}] %}
        - cell 1
        - cell 2
        {% /if-pref %}
        `,
    `
        {% if $fakeCondition.condition1 %}
        - cell 1
        - cell 2
        {% else /%}
        - cell 3
        - cell 4
        {% /if %}
        `,
    `
        {% unregistered-if-tag $fakeCondition.condition2 %}
        - cell 1
        - cell 2
        {% /unregistered-if-tag %}
        `,
  ]),
};

// table.test.ts's inputs (Markdoc.parse(string): `new Tokenizer()`, unless said).
const tables = {
  'table: non-row content': `{% table %}
* Heading 1
* Heading 2
---
* Cell 1
* Cell 2
{% if $foo %}
This is invalid conditional content at the row level of the table.
{% /if %}
This is invalid non-conditional content at the row level.
{% /table %}`,
  'table: valid conditional rows': `{% table %}
* Heading 1
* Heading 2
---
{% if $foo %}
* Row 1 Cell 1
* Row 1 Cell 2
{% /if %}
---
{% if $bar %}
* Row 2 Cell 1
* Row 2 Cell 2
{% else /%}
* Alt Row 2 Cell 1
* Alt Row 2 Cell 2
{% /if %}
{% /table %}`,
  'table: conditionals within a cell': `{% table %}
* Heading 1
* Heading 2
---
* Cell 1
* Cell 2
  {% if $foo %}
  This is a conditional paragraph inside cell 2.
  {% else /%}
  This is an alternate paragraph inside cell 2.
  {% /if %}
{% /table %}`,
  'table: conditional with rows and hrs': `{% table %}
* Heading 1
* Heading 2
---
{% if $foo %}
* Row 1 Cell 1
* Row 1 Cell 2
---
* Row 2 Cell 1
* Row 2 Cell 2
{% /if %}
{% /table %}`,
  'table: invalid tag in a conditional': `{% table %}
* Heading 1
* Heading 2
---
{% if $foo %}
{% callout %}
This is not a valid row
{% /callout %}
{% /if %}
{% /table %}`,
  'table: non-conditional tag at row level': `{% table %}
* Heading 1
* Heading 2
---
{% callout %}
This is not a valid row
{% /callout %}
{% /table %}`,
};

// This crate's own cases: positions, frontmatter, JS-object quirks.
const PLUGINS = { allowIndentation: true, allowComments: true };
const own = {
  'pos: heading and prose': { src: '# Heading\n\nSome prose.\n' },
  'pos: frontmatter': { src: '---\ntitle: x\nfoo: bar\n---\n\n# Heading\n' },
  'pos: two inline tags': { src: '{% tag id="a" /%} and {% tag id="b" /%}\n' },
  'pos: prose after a tag': { src: '{% callout type="note" %}\nInside the callout.\n{% /callout %}\n\nAfter.\n' },
  'pos: escaped tag opener': { src: 'Literal \\{% not a tag %} here.\n\nAfter.\n' },
  'pos: utf-16 columns': { src: 'café **bold**\n' },
  'pos: byte-order mark kept': { src: '﻿# Heading\n\nText.\n' },
  'pos: byte-order mark stripped': { src: '﻿# Heading\n\nText.\n', tokenize: '# Heading\n\nText.\n' },
  'pos: interpolation': { src: 'The {% $bot250 %} is heavier.\n' },
  'pos: list table': { src: 'Before.\n\n{% table %}\n---\n* A\n* B\n{% /table %}\n' },
  'pos: em dash in a list table': { src: '{% table %}\n---\n* Connected to the robot—see below\n{% /table %}\n' },
  'pos: unclosed tag': { src: '# T\n\n{% callout %}\nbody\n' },
  'pos: close with nothing open': { src: '# T\n\nText.\n\n{% /callout %}\n' },
  'pos: inline constructs': {
    src: 'A *b {% x /%}* `c {% y %}` [d {% z /%}](/u "t") <http://a.b> \\* &amp; {% w /%}\n',
  },
  'pos: softbreak and hardbreak': { src: 'one  \n   two\\\nthree\n four {% t /%}\n' },
  'pos: blockquote and list': { src: '> - a {% t1 /%}\n>   b {% t2 /%}\n>\n> {% t3 %}\n> c\n> {% /t3 %}\n', options: PLUGINS },
  'pos: pipe table': { src: '> | a | {% x /%} |\n> |---|---|\n> | b \\| c | d {% y /%} |\n' },
  'pos: fence with tags': { src: '- item\n\n  ```js {% .c %}\n  a {% t1 /%}\n    {% t2 /%}\n  ```\n' },
  'pos: heading with closing hashes': { src: '## Title {% #id %} ##\n' },
  'pos: crlf': { src: '# A\r\n\r\npara {% t /%}\r\nnext {% u /%}\r\n' },
  'pos: multi-line inline tag': { src: '> a {% t\n> x=1 %} b\n' },
  'pos: reference link': { src: '[a][r] {% t /%} [b] {% u /%}\n\n[r]: /x\n[b]: /y\n' },
  'pos: entity and smart quotes': { src: '"Hi" &copy; (c) -- {% t /%}\n', options: { typographer: true } },
  'pos: nbsp at line end': { src: '2. Depending\u00a0\n   {% t /%}\n' },
  'pos: degenerate tags': { src: 'a {%} b {%%} c\n\n{%}\n' },
  'pos: emptied delimiter': { src: '{% s %} *a **b** *c*\n' },
  'fm: valid': { src: '---\nb: 1\na: [x, 2]\nc:\n  d: true\n---\n# T\n' },
  'fm: invalid': { src: '---\nFoo\n---\nBar\n' },
  'fm: empty': { src: '---\n---\n# T\n' },
  'quirk: unclosed inline tag': { src: 'a {% foo %} b\n\nc\n' },
  'quirk: class onto a string': { src: '{% foo class="x" .bar /%}\n' },
  'quirk: class onto values': { src: '{% foo class={a: 1} .bar .2 /%}\n{% foo class=[1,2] .length .3 /%}\n{% foo class=null .bar /%}\n' },
  'quirk: prototype names': { src: '{% foo constructor=1 __proto__=2 toString=3 /%}\n{% foo .constructor .__proto__ /%}\n' },
  'quirk: slots': { src: '{% foo %}\n{% slot "b" %}\nB\n{% /slot %}\n{% slot "0" %}\nZ\n{% /slot %}\n{% slot "b" %}\nB2\n{% /slot %}\n{% slot "__proto__" %}\nP\n{% /slot %}\n{% /foo %}\n' },
  'quirk: parse errors': { src: '{% bad = %}\n\na {% bad = %} b\n' },
  'quirk: link close inside a tag': { src: '[a {% t %}b](/u)\n' },
  // An annotation shares the object `class` holds (parser.rs, `annotate`).
  'alias: class object': { src: '{% x class={a: 1} .b /%}\n' },
  'alias: class array length': { src: '{% x class=[1, 2, 3] .length /%}\n' },
  'alias: inline annotations': { src: 'Para {% class={a: true} %} more {% .b %}\n' },
  'alias: class replaced': { src: '{% x class={a: 1} .b class={c: 2} .d /%}\n{% x .b .c /%}\n{% x class=[1] .2 class=null .d /%}\n' },
  'alias: tag as inline parent': { src: '* {% x class={a: 1} %}\n  item {% .b %}\n* y\n' },
  // `.name` onto a string, number or boolean `class`: one document each, as each throws.
  'primitive: string length': { src: '{% x class="abc" .length /%}\n' },
  'primitive: string index': { src: '{% x class="abc" .2 /%}\n' },
  'primitive: string past its end': { src: '{% x class="abc" .3 /%}\n' },
  'primitive: string utf-16 index': { src: '{% x class="\u{1F600}" .1 /%}\n' },
  'primitive: string non-canonical index': { src: '{% x class="abc" .01 /%}\n' },
  'primitive: string minus zero': { src: '{% x class="abc" .-0 /%}\n' },
  'primitive: number length': { src: '{% x class=1.5 .length /%}\n' },
  'primitive: number in exponent form': { src: '{% x class=1000000000000000000000 .b /%}\n' },
  'primitive: boolean index': { src: '{% x class=true .0 /%}\n' },
  'primitive: __proto__': { src: '{% x class="s" .__proto__ /%}\n{% x class=1 .__proto__ /%}\n{% x class=true .__proto__ /%}\n' },
  // Holes (parser.rs, `MAX_ARRAY_HOLES`).
  'holes: at the cap': { src: '{% x class=[] .65536 /%}\n' },
  'holes: past the cap': { src: '{% x class=[] .65537 /%}\n' },
  'holes: the largest index': { src: '{% x class=[] .4294967294 /%}\n' },
  'holes: the cap is per parse': { src: '{% x class=[] .40000 /%}\n\n{% y class=[] .30000 /%}\n' },
  // Not modeled (parser.rs's module docs): the port's behaviour, pinned.
  'not modeled: __proto__ attributes': { src: '{% x __proto__={a: 1} a=2 /%}\n{% x __proto__=null constructor=1 /%}\n{% x __proto__={class: "s"} .y /%}\n' },
  'not modeled: __proto__ slot': { src: '{% x %}\n{% slot __proto__={primary: "p"} %}\nhi\n{% /slot %}\n{% /x %}\n' },
  'not modeled: class on objects JSON cannot show': { src: '{% x class=$v .b /%}\n{% x class=f() .b /%}\n{% x class=["a"] .b /%}\n' },
  // The table transform's walk (transforms/table.rs): a slot's table, a table in a cell.
  'table: in a slot and in a cell': { src: '{% x %}\n{% slot "s" %}\n{% table %}\n- a\n---\n- {% table %}\n  - c\n  ---\n  - d\n  {% /table %}\n{% /table %}\n{% /slot %}\n{% /x %}\n', options: PLUGINS },
  // Positions: tags beside escapes (positions.rs).
  'pos: escaped backtick before a code span': { src: '\\``a` {% x /%} `b`\n' },
  'pos: escaped pipe in a cell': { src: '| {% x /%}\\| |\n|---|\n' },
  'pos: NUL and an escaped pipe in a cell': { src: '| a\\|{% x /%}\u0000 |\n|---|\n' },
};

// Cases whose upstream AST is kept beside their tokens, for the tests that pin the port to
// node's output: `ast` is upstream's JSON of `Markdoc.parse(tokens, {slots, location: true,
// ...args})`, or `threw` its error (`${name}: ${message}`). `tokens` given (hand-made) are parsed as they
// are; otherwise `src` is tokenized with `options` (default: both switches off).
const textToken = (content) => ({ type: 'text', tag: '', attrs: null, map: null, nesting: 0, level: 0, children: null, content, markup: '', info: '', meta: null, block: false, hidden: false });
function nestedInline(depth, leaf) {
  let t = leaf;
  for (let i = 0; i < depth; i++) t = { ...textToken(''), type: 'inline', children: [t] };
  return [t];
}
const oracle = {
  // An own `__proto__` key (an object literal's) is set by `.__proto__` as any other name.
  'proto: own key': { src: '{% x class={__proto__: 1} .__proto__ /%}\n' },
  'proto: own string key': { src: '{% x class={"__proto__": {"a": 1}} .__proto__ /%}\n' },
  'proto: own key, inline': { src: 'p {% class={__proto__: 1} %} {% .__proto__ %}\n' },
  'proto: own key, inline twice': { src: 'p {% class={"__proto__": 1} %} {% .__proto__ %} and {% .__proto__ %}\n' },
  'proto: own key keeps its place': { src: '{% x class={a: 1, "__proto__": 2, b: 3} .__proto__ .c /%}\n' },
  'proto: own key twice': { src: '{% x class={__proto__: 1} .__proto__ .__proto__ /%}\n' },
  'proto: computed key, then again': { src: '{% x .__proto__ .__proto__ .b /%}\n' },
  'proto: own null': { src: '{% x class={"__proto__": null} .__proto__ /%}\n' },
  'proto: own array': { src: '{% x class={__proto__: [1]} .__proto__ /%}\n' },
  'proto: nested own key untouched': { src: '{% x class={a: {__proto__: 1}} .__proto__ /%}\n' },
  'proto: own key, class replaced': { src: '{% x class={__proto__: 1} .__proto__ class={__proto__: 2} .__proto__ /%}\n' },
  'proto: own key beside an index': { src: '{% x class={0: 1, __proto__: 2} .0 .__proto__ .1 /%}\n' },
  'proto: own key, tag as inline parent': { src: '* {% x class={__proto__: 1} %}\n  item {% .__proto__ %}\n* y\n' },
  'proto: own key in a heading': { src: '# T {% class={__proto__: 1} %} {% .__proto__ %}\n' },
  'proto: own key on a fence': { src: '```js {% class={__proto__: 1} .__proto__ %}\nx\n```\n' },
  'proto: no own key': { src: '{% x class={a: 1} .__proto__ /%}\n' },
  'proto: own key holding an object': { src: '{% x class={__proto__: {a: 1}} .b /%}\n' },
  'proto: own key after others': { src: '{% x class={b: 1, __proto__: {x: 5}} .x /%}\n' },
  'proto: own key in an attribute': { src: '{% x a={__proto__: 1} /%}\n' },
  'proto: own key in an array class': { src: '{% x class=[{__proto__: 1}] .0 .__proto__ /%}\n' },
  'proto: own key as a primary': { src: '{% x {__proto__: 2} /%}\n' },
  'proto: a parameter named __proto__': { src: '{% x f(__proto__=1) /%}\n' },
  // Hand-made tokens nested in `children` (Markdoc's tokenizer nests two deep at most).
  'nest: inline tokens 3 deep': { tokens: nestedInline(3, textToken('x')) },
  'nest: a hidden token 3 deep': { tokens: nestedInline(3, { ...textToken('x'), hidden: true }) },
  'nest: an open token 3 deep': { tokens: nestedInline(3, { ...textToken(''), type: 'paragraph_open', nesting: 1 }) },
  // The shapes that nest: an inline tag left open leaves its inline and block open.
  'depth: paragraphs': { src: 'p {% x %} q\n\np {% x %} q\n\np {% x %} q\n' },
  'depth: tight items': { src: '* a {% x %} b\n* a {% x %} b\n* a {% x %} b\n' },
  // Upstream's `lines` and `location`: token maps, a closing's map pushed onto the array the
  // tag's node (and every node sharing it) holds, the fence tags' `position`, the file.
  'pos: a closing on its own lines': { src: '# Hi {% .a %}\n\n{% callout type="x" %}\nSome *em* text\n{% /callout %}\n', args: { file: 'f.md' } },
  'pos: tags in a fence': { src: '```\nx {% a %} y {% $v %}\n{% /a %}\n```\n', args: { file: 'f.md' } },
  'pos: an inline tag closed inline': { src: 'a {% x %} b {% /x %} c\n' },
  'pos: location off, a closing still pushes': { src: '{% x %}\nA\n{% /x %}\n', args: { location: false } },
  'pos: hand-made positions': { tokens: [
    { ...textToken('a'), position: { start: { b: 1, 1: 2 }, end: null } },
    { ...textToken('b'), position: 'abc' },
    { ...textToken('c'), position: [1, 2], map: [4, 9] },
    { ...textToken('d'), position: { end: 3 } },
  ] },
  'pos: a null position throws': { tokens: [{ ...textToken('a'), position: null }] },
  'pos: a map-less token shares the lines a closing grows': { tokens: [
    { ...textToken(''), type: 'tag_open', nesting: 1, map: [1, 2], meta: { tag: 'x' } },
    textToken('before'),
    { ...textToken(''), type: 'tag_close', nesting: -1, map: [7, 8], meta: { tag: 'x' } },
    textToken('after'),
  ] },
  'pos: a token pushed twice shares the grown lines': { tokens: [
    { ...textToken(''), type: 'tag_open', nesting: 1, meta: { tag: 'x' }, children: [
      textToken('one'),
      { ...textToken(''), type: 'tag_close', nesting: -1, map: [5, 6], meta: { tag: 'x' } },
      textToken('two'),
    ] },
  ] },
  // Once a hand-made `document_close` pops the document, the next token reads a property of
  // `undefined`: which one depends on the token.
  'pos: after the document closes, a node': { tokens: [
    { ...textToken(''), type: 'document_close', nesting: -1, map: [0, 1] },
    { ...textToken('x'), map: [1, 2] },
  ] },
  'pos: after the document closes, a map-less node': { tokens: [
    { ...textToken(''), type: 'document_close', nesting: -1 },
    textToken('x'),
  ] },
  'pos: after the document closes, a closing': { tokens: [
    { ...textToken(''), type: 'document_close', nesting: -1 },
    { ...textToken(''), type: 'paragraph_close', nesting: -1 },
  ] },
  'pos: after the document closes, an annotation': { tokens: [
    { ...textToken(''), type: 'document_close', nesting: -1 },
    { ...textToken(''), type: 'annotation', meta: { attributes: [] } },
  ] },
  'pos: after the document closes, a slot': { slots: true, tokens: [
    { ...textToken(''), type: 'document_close', nesting: -1 },
    { ...textToken(''), type: 'tag', map: [2, 3], meta: { tag: 'slot', attributes: [{ type: 'attribute', name: 'primary', value: 'p' }] } },
  ] },
  'pos: after the document closes, a hidden token': { tokens: [
    { ...textToken(''), type: 'document_close', nesting: -1, map: [3, 4] },
    { ...textToken('x'), hidden: true },
  ] },
  // A node's errors: the token's own entries as they are, and the parse error of `meta.error`.
  'errors: a token without meta.error': { tokens: [{ ...textToken(''), type: 'error' }] },
  'errors: a null meta.error': { tokens: [{ ...textToken(''), type: 'error', meta: { error: null } }] },
  'errors: a primitive meta.error': { tokens: [{ ...textToken(''), type: 'error', meta: { error: 7 } }] },
  'errors: a meta.error without a message': { tokens: [{ ...textToken(''), type: 'error', map: [0, 1], meta: { error: { location: null } } }] },
  'errors: a meta.error with an object message': { tokens: [{ ...textToken(''), type: 'error', meta: { error: { message: { b: 1, 0: 2 }, location: { start: { line: 1 }, end: { line: 2 } } } } }] },
  'errors: hand-made entries': { tokens: [{ ...textToken('x'), errors: [
    5, null, 's', [1, 2],
    { id: 'e', level: 'bogus', message: 'm' },
    { message: 'm', id: 'e', level: 'error' },
    { id: 1, level: 'error', message: 'm' },
    { id: 'e', level: 'error', message: 'm', x: 1, 1: 0 },
    { id: 'e', level: 'error' },
    { id: 'e', level: 'warning', message: 'm', location: { end: { line: 4 }, start: { line: 3 } } },
  ] }] },
};

const out = {};
const tokenizers = new Map();
function tokenize(text, options) {
  const key = JSON.stringify(options);
  if (!tokenizers.has(key)) tokenizers.set(key, new Markdoc.Tokenizer(options));
  return JSON.parse(JSON.stringify(tokenizers.get(key).tokenize(text)));
}
for (const [name, example] of Object.entries(upstream)) {
  const src = example.replace(/\n\s+/gm, '\n').trim();
  out[name] = { src, tokens: tokenize(src, { allowComments: true }) };
}
for (const [name, src] of Object.entries(tables)) out[name] = { src, tokens: tokenize(src, {}) };
out['table: comments in a table'] = {
  src: `{% table %}
* Heading 1
* Heading 2
---
{% comment %}
comment row
{% /comment %}
* Cell 1
* Cell 2
---
{% if $foo %}
{% comment %}
comment inside conditional
{% /comment %}
* Row Cell 1
* Row Cell 2
{% /if %}
{% /table %}`,
};
out['table: comments in a table'].tokens = tokenize(out['table: comments in a table'].src, { allowComments: true });
for (const [name, c] of Object.entries(own))
  out[name] = { src: c.src, tokens: tokenize(c.tokenize ?? c.src, c.options ?? {}) };

for (const [name, c] of Object.entries(oracle)) {
  const options = c.options ?? {};
  const tokens = c.tokens ? JSON.parse(JSON.stringify(c.tokens)) : null;
  const src = c.src ?? '';
  // Parsed from a copy: upstream's parser pushes onto a token's `map` and `errors`.
  const live = tokens ? JSON.parse(JSON.stringify(tokens)) : (() => {
    const key = JSON.stringify(options);
    if (!tokenizers.has(key)) tokenizers.set(key, new Markdoc.Tokenizer(options));
    return tokenizers.get(key).tokenize(src);
  })();
  const entry = { src, tokens: tokens ?? tokenize(src, options) };
  if (c.slots) entry.slots = true;
  // More ParserArgs (`file`, `location`), passed on as they are.
  if (c.args) entry.args = c.args;
  try {
    entry.ast = JSON.parse(JSON.stringify(Markdoc.parse(live, { slots: !!c.slots, location: true, ...(c.args ?? {}) })));
  } catch (e) {
    entry.threw = `${e.name}: ${e.message}`;
  }
  out[name] = entry;
}

// One case per line.
const lines = Object.entries(out).map(([k, v]) => `${JSON.stringify(k)}: ${JSON.stringify(v)}`);
process.stdout.write(`{\n${lines.join(',\n')}\n}\n`);
