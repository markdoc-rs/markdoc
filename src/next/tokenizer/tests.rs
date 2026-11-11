//! Upstream's tests of the tokenizer plugins (`src/tokenizer/plugins/*.test.ts`), ported:
//! each `toDeepEqualSubset` is [`subset`] over the tokens as JSON (`plain()`'s form). Then
//! what those tests do not reach: what `tokenize` throws, and the stack it needs. Expected
//! values that are not upstream's own were printed by node from upstream's dist.

use serde_json::{Value, json};

use super::{LinkPluginOptions, TokenizeError, Tokenizer, TokenizerOptions};
use crate::grammar::MAX_NESTING;
use crate::markdown_it::{Env, Token};
use crate::next::compare::tokens_json;

/// upstream's `example.replace(/\n\s+/gm, '\n').trim()`: a newline followed by whitespace
/// (newlines included, so blank lines go too) becomes one newline.
fn dedent(example: &str) -> String {
    let mut out = String::new();
    let mut chars = example.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if c == '\n' {
            while chars.peek().is_some_and(|&c| is_js_space(c)) {
                chars.next();
            }
        }
    }
    let units = crate::markdown_it::js::to_utf16(&out);
    crate::markdown_it::js::to_string(crate::markdown_it::js::trim(&units))
}

fn is_js_space(c: char) -> bool {
    crate::markdown_it::js::is_js_space(c as u32)
}

fn tokenizer(options: TokenizerOptions) -> Tokenizer {
    Tokenizer::new(options)
}

fn parse_with(t: &Tokenizer, example: &str) -> Vec<Token> {
    t.tokenize(&dedent(example)).expect("tokenize")
}

fn parse(example: &str) -> Vec<Token> {
    parse_with(&Tokenizer::default(), example)
}

/// `expect(actual).toDeepEqualSubset(expected)`: every key of an expected object is in the
/// actual one with a matching value (`null`, and `false` for `hidden`, also match a missing
/// key, as `plain()` leaves those out), arrays item by item and of the same length.
fn subset(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => {
            for (k, ev) in e {
                let p = format!("{path}.{k}");
                match a.get(k) {
                    Some(av) => subset(av, ev, &p),
                    None => assert!(
                        ev.is_null() || (k == "hidden" && ev == &json!(false)),
                        "{p}: missing, expected {ev}"
                    ),
                }
            }
        }
        (Value::Array(a), Value::Array(e)) => {
            assert_eq!(a.len(), e.len(), "{path}: length of {actual}");
            for (i, (av, ev)) in a.iter().zip(e).enumerate() {
                subset(av, ev, &format!("{path}[{i}]"));
            }
        }
        (Value::Number(a), Value::Number(e)) => assert_eq!(a.as_f64(), e.as_f64(), "{path}"),
        _ => assert_eq!(actual, expected, "{path}"),
    }
}

fn expect(tokens: &[Token], expected: Value) {
    subset(&tokens_json(tokens), &expected, "tokens");
}

fn map(t: &Token) -> Option<[usize; 2]> {
    t.map
}

// --- annotations.test.ts ------------------------------------------------------------------

#[test]
fn with_a_simple_container() {
    let example = parse(
        "
      {% test %}
      This is a test
      {% /test %}
      ",
    );
    expect(
        &example,
        json!([
            {"type": "tag_open", "info": "test", "nesting": 1, "block": true, "children": null,
             "hidden": false, "map": [0, 1], "meta": {"tag": "test", "attributes": null}},
            {"type": "paragraph_open"},
            {"type": "inline"},
            {"type": "paragraph_close"},
            {"type": "tag_close", "info": "/test", "nesting": -1, "block": true, "children": null,
             "hidden": false, "map": [2, 3], "meta": {"tag": "test"}},
        ]),
    );
}

#[test]
fn with_an_id_and_class() {
    let example = parse(
        "
      {% test #foo .bar %}
      This is a test
      {% /test %}",
    );
    expect(
        &example,
        json!([
            {"type": "tag_open", "meta": {"tag": "test", "attributes": [
                {"type": "attribute", "name": "id", "value": "foo"},
                {"type": "class", "name": "bar", "value": true},
            ]}},
            {"type": "paragraph_open"},
            {"type": "inline"},
            {"type": "paragraph_close"},
            {"type": "tag_close"},
        ]),
    );
}

#[test]
fn with_a_self_closing_container() {
    let example = parse("This is a test\n{% test /%}");
    expect(
        &example,
        json!([
            {"type": "paragraph_open"},
            {"type": "inline"},
            {"type": "paragraph_close"},
            {"type": "tag", "meta": {"tag": "test", "attributes": null}},
        ]),
    );

    let example2 = parse("{% test /%}");
    expect(
        &example2,
        json!([{"type": "tag", "meta": {"tag": "test", "attributes": null}}]),
    );
}

#[test]
fn with_a_self_closing_container_with_annotations() {
    let example = parse(
        "
      This is a test
      {% test #foo .bar baz=1 /%}
      This is another test
      ",
    );
    expect(
        &example,
        json!([
            {"type": "paragraph_open"},
            {"type": "inline"},
            {"type": "paragraph_close"},
            {"type": "tag", "meta": {"tag": "test", "attributes": [
                {"type": "attribute", "name": "id", "value": "foo"},
                {"type": "class", "name": "bar", "value": true},
                {"type": "attribute", "name": "baz", "value": 1},
            ]}},
            {"type": "paragraph_open"},
            {"type": "inline"},
            {"type": "paragraph_close"},
        ]),
    );
}

fn basic_example() -> Vec<Value> {
    vec![
        json!({"type": "tag_open", "nesting": 1, "meta": {"attributes": [
            {"type": "attribute", "name": "id", "value": "foo"},
            {"type": "class", "name": "bar", "value": true},
            {"type": "attribute", "name": "baz", "value": 1},
        ], "tag": "test"}}),
        json!({"type": "paragraph_open", "tag": "p", "nesting": 1}),
        json!({"type": "inline", "nesting": 0,
               "children": [{"type": "text", "nesting": 0, "content": "This is a test"}]}),
        json!({"type": "paragraph_close", "tag": "p", "nesting": -1}),
        json!({"type": "tag_close", "nesting": -1, "meta": {"tag": "test"}}),
    ]
}

fn around(before: Vec<Value>, after: Vec<Value>) -> Value {
    Value::Array(
        before
            .into_iter()
            .chain(basic_example())
            .chain(after)
            .collect(),
    )
}

#[test]
fn multiline_basic() {
    let example = parse(
        "
        {% test #foo .bar
          baz=1 %}
        This is a test
        {% /test %}
        ",
    );
    expect(&example, around(vec![], vec![]));
    assert_eq!(map(&example[0]), Some([0, 2]));
    assert_eq!(map(&example[1]), Some([2, 3]));
    assert_eq!(map(&example[4]), Some([3, 4]));
    assert_eq!(example.len(), 5);
    assert_eq!(example[2].children.as_ref().map(Vec::len), Some(1));
}

#[test]
fn multiline_basic_with_symbols_on_separate_lines() {
    let example = parse(
        "
        {%
          test #foo .bar
          baz=1
        %}
        This is a test
        {% /test %}
        ",
    );
    expect(&example, around(vec![], vec![]));
    assert_eq!(map(&example[0]), Some([0, 4]));
    assert_eq!(map(&example[2]), Some([4, 5]));
    assert_eq!(map(&example[4]), Some([5, 6]));
    assert_eq!(example.len(), 5);
    assert_eq!(example[2].children.as_ref().map(Vec::len), Some(1));
}

#[test]
fn multiline_inside_a_blockquote() {
    let example = parse(
        "
        > {% test #foo .bar
        >   baz=1 %}
        > This is a test
        > {% /test %}
        ",
    );
    expect(
        &example,
        around(
            vec![json!({"type": "blockquote_open", "nesting": 1})],
            vec![json!({"type": "blockquote_close", "nesting": -1})],
        ),
    );
    assert_eq!(map(&example[1]), Some([0, 2]));
    assert_eq!(map(&example[2]), Some([2, 3]));
    assert_eq!(map(&example[5]), Some([3, 4]));
    assert_eq!(example.len(), 7);
}

#[test]
fn multiline_followed_by_content_after_the_blockquote() {
    let example = parse(
        "
        > {% test #foo .bar
        >   baz=1 %}
        > This is a test
        > {% /test %}
        After the blockquote
        ",
    );
    expect(
        &example,
        around(
            vec![json!({"type": "blockquote_open", "nesting": 1})],
            vec![
                json!({"type": "blockquote_close", "nesting": -1}),
                json!({"type": "paragraph_open", "tag": "p", "nesting": 1}),
                json!({"type": "inline", "nesting": 0, "children": [
                    {"type": "text", "nesting": 0, "content": "After the blockquote"}]}),
                json!({"type": "paragraph_close", "tag": "p", "nesting": -1}),
            ],
        ),
    );
    assert_eq!(map(&example[1]), Some([0, 2]));
    assert_eq!(map(&example[5]), Some([3, 4]));
    assert_eq!(map(&example[7]), Some([4, 5]));
    assert_eq!(example.len(), 10);
}

#[test]
fn multiline_inside_a_list_item() {
    // Not dedented: list continuation depends on the indentation
    let example = Tokenizer::default()
        .tokenize("- {% test #foo .bar\n    baz=1 %}\n  This is a test\n  {% /test %}")
        .unwrap();
    expect(
        &example,
        around(
            vec![
                json!({"type": "bullet_list_open", "nesting": 1}),
                json!({"type": "list_item_open", "nesting": 1}),
            ],
            vec![
                json!({"type": "list_item_close", "nesting": -1}),
                json!({"type": "bullet_list_close", "nesting": -1}),
            ],
        ),
    );
    assert_eq!(map(&example[2]), Some([0, 2]));
    assert_eq!(map(&example[3]), Some([2, 3]));
    assert_eq!(map(&example[6]), Some([3, 4]));
    assert_eq!(example.len(), 9);
}

#[test]
fn multiline_inside_a_list_item_inside_a_blockquote() {
    let example = Tokenizer::default()
        .tokenize("> - {% test #foo .bar\n>     baz=1 %}\n>   This is a test\n>   {% /test %}")
        .unwrap();
    expect(
        &example,
        around(
            vec![
                json!({"type": "blockquote_open", "nesting": 1}),
                json!({"type": "bullet_list_open", "nesting": 1}),
                json!({"type": "list_item_open", "nesting": 1}),
            ],
            vec![
                json!({"type": "list_item_close", "nesting": -1}),
                json!({"type": "bullet_list_close", "nesting": -1}),
                json!({"type": "blockquote_close", "nesting": -1}),
            ],
        ),
    );
    assert_eq!(map(&example[3]), Some([0, 2]));
    assert_eq!(map(&example[4]), Some([2, 3]));
    assert_eq!(map(&example[7]), Some([3, 4]));
    assert_eq!(example.len(), 11);
}

#[test]
fn with_a_single_line_container_inside_a_blockquote() {
    let example = parse(
        "
      > {% test #foo .bar baz=1 %}
      > This is a test
      > {% /test %}
      ",
    );
    expect(
        &example,
        around(
            vec![json!({"type": "blockquote_open", "nesting": 1})],
            vec![json!({"type": "blockquote_close", "nesting": -1})],
        ),
    );
    assert_eq!(map(&example[1]), Some([0, 1]));
    assert_eq!(map(&example[2]), Some([1, 2]));
    assert_eq!(map(&example[5]), Some([2, 3]));
    assert_eq!(example.len(), 7);
}

#[test]
fn inline_on_a_line_by_itself() {
    let example = parse("{% foo %}bar{% /foo %}");
    expect(
        &example,
        json!([
            {"type": "paragraph_open"},
            {"type": "inline", "children": [
                {"type": "tag_open", "info": "foo", "nesting": 1, "meta": {"tag": "foo"}},
                {"type": "text", "content": "bar"},
                {"type": "tag_close", "info": "/foo"},
            ]},
            {"type": "paragraph_close"},
        ]),
    );
}

#[test]
fn inline_with_a_paragraph() {
    let example = parse("Example {% foo %}bar{% /foo %} baz");
    expect(
        &example,
        json!([
            {"type": "paragraph_open"},
            {"type": "inline", "children": [
                {"type": "text", "content": "Example "},
                {"type": "tag_open", "info": "foo", "nesting": 1, "block": false,
                 "children": null, "hidden": false, "meta": {"attributes": null, "tag": "foo"}},
                {"type": "text", "content": "bar"},
                {"type": "tag_close", "info": "/foo", "nesting": -1, "block": false,
                 "children": null, "hidden": false, "meta": {"tag": "foo"}},
                {"type": "text", "content": " baz"},
            ]},
            {"type": "paragraph_close"},
        ]),
    );
}

#[test]
fn inline_with_two_in_succession() {
    let example = parse("Example {% foo %}bar{% /foo %}{% test %}test{% /test %} baz");
    expect(
        &example,
        json!([
            {"type": "paragraph_open"},
            {"type": "inline", "children": [
                {"type": "text", "content": "Example "},
                {"type": "tag_open", "info": "foo", "nesting": 1,
                 "meta": {"attributes": null, "tag": "foo"}},
                {"type": "text", "content": "bar"},
                {"type": "tag_close", "info": "/foo", "nesting": -1, "meta": {"tag": "foo"}},
                {"type": "tag_open", "info": "test", "nesting": 1,
                 "meta": {"attributes": null, "tag": "test"}},
                {"type": "text", "content": "test"},
                {"type": "tag_close", "info": "/test", "nesting": -1, "meta": {"tag": "test"}},
                {"type": "text", "content": " baz"},
            ]},
            {"type": "paragraph_close"},
        ]),
    );
}

#[test]
fn inline_with_markdown_inside() {
    let example = parse("Example {% foo %}this is a *test*{% /foo %} baz");
    expect(
        &example,
        json!([
            {"type": "paragraph_open"},
            {"type": "inline", "children": [
                {"type": "text", "content": "Example "},
                {"type": "tag_open", "info": "foo", "nesting": 1,
                 "meta": {"attributes": null, "tag": "foo"}},
                {"type": "text", "content": "this is a "},
                {"type": "em_open"},
                {"type": "text", "content": "test"},
                {"type": "em_close"},
                {"type": "tag_close", "info": "/foo", "nesting": -1, "meta": {"tag": "foo"}},
                {"type": "text", "content": " baz"},
            ]},
            {"type": "paragraph_close"},
        ]),
    );
}

#[test]
fn fence_simple_with_no_tags() {
    let example = parse("```\nhello\n```");
    expect(
        &example,
        json!([{"type": "fence", "children": [{"type": "text", "content": "hello\n"}]}]),
    );
}

#[test]
fn fence_simple_with_one_tag() {
    let example = parse("```\nhello {% foo %}bar{% /foo %}\n```");
    expect(
        &example,
        json!([{"type": "fence", "children": [
            {"type": "text", "content": "hello "},
            {"type": "tag_open", "info": "{% foo %}"},
            {"type": "text", "content": "bar"},
            {"type": "tag_close", "info": "{% /foo %}"},
            {"type": "text", "content": "\n"},
        ]}]),
    );
}

#[test]
fn fence_unclosed_tag() {
    // unclosed tags should not result in crashes
    let example = parse("```\nhello {%\n```");
    expect(
        &example,
        json!([{"type": "fence", "children": [{"type": "text", "content": "hello {%\n"}]}]),
    );
}

#[test]
fn inline_annotations_with_a_header() {
    let example = parse("# This is a test {% #foo .bar .baz %}");
    expect(
        &example,
        json!([
            {"type": "heading_open"},
            {"type": "inline", "children": [
                {"type": "text", "content": "This is a test "},
                {"type": "annotation", "meta": {"attributes": [
                    {"type": "attribute", "name": "id", "value": "foo"},
                    {"type": "class", "name": "bar", "value": true},
                    {"type": "class", "name": "baz", "value": true},
                ]}},
            ]},
            {"type": "heading_close"},
        ]),
    );
    assert!(
        example[1].children.as_ref().unwrap()[1]
            .meta
            .as_ref()
            .unwrap()
            .get("tag")
            .is_none()
    );
}

#[test]
fn inline_annotations_with_a_header_and_keys() {
    let example = parse("# This is a test {% #foo .bar .baz foo=2 %}");
    expect(
        &example,
        json!([
            {"type": "heading_open"},
            {"type": "inline", "children": [
                {"type": "text", "content": "This is a test "},
                {"type": "annotation", "meta": {"attributes": [
                    {"type": "attribute", "name": "id", "value": "foo"},
                    {"type": "class", "name": "bar", "value": true},
                    {"type": "class", "name": "baz", "value": true},
                    {"type": "attribute", "name": "foo", "value": 2},
                ]}},
            ]},
            {"type": "heading_close"},
        ]),
    );
}

// --- comments.test.ts -----------------------------------------------------------------------

fn with_comments() -> Tokenizer {
    tokenizer(TokenizerOptions {
        allow_comments: true,
        ..TokenizerOptions::default()
    })
}

fn inline_comment_output() -> Value {
    json!([
        {"type": "paragraph_open"},
        {"type": "inline", "children": [
            {"type": "text", "content": "this is a test "},
            {"type": "comment", "content": "example comment"},
            {"type": "text", "content": " foo"},
        ]},
        {"type": "paragraph_close"},
    ])
}

#[test]
fn simple_inline_comment() {
    let example = parse_with(
        &with_comments(),
        "
      this is a test <!-- example comment --> foo
      ",
    );
    expect(&example, inline_comment_output());
}

#[test]
fn inline_comment_with_a_newline() {
    let example = parse_with(
        &with_comments(),
        "
      this is a test <!--
        example comment
        --> foo
      ",
    );
    expect(&example, inline_comment_output());
}

fn block_comment_output() -> Value {
    json!([
        {"type": "paragraph_open"},
        {"type": "inline"},
        {"type": "paragraph_close"},
        {"type": "comment", "content": "example comment"},
        {"type": "paragraph_open"},
        {"type": "inline", "content": "foo"},
        {"type": "paragraph_close"},
    ])
}

#[test]
fn simple_block_comment_after_a_paragraph() {
    let example = parse_with(
        &with_comments(),
        "
      this is a test

      <!--
      example comment
      -->

      foo
      ",
    );
    expect(&example, block_comment_output());
}

#[test]
fn block_comment_with_ending_on_same_line_as_content() {
    let example = parse_with(
        &with_comments(),
        "
      this is a test

      <!--
      example comment -->

      foo
      ",
    );
    expect(&example, block_comment_output());
}

#[test]
fn block_comment_on_one_line() {
    let example = parse_with(
        &with_comments(),
        "
      this is a test

      <!-- example comment -->

      foo
      ",
    );
    expect(&example, block_comment_output());
}

#[test]
fn block_comment_across_multiple_lines_with_blank_lines() {
    let actual = with_comments()
        .tokenize(
            "
      foo <!-- example

      comment --> bar
      "
            .trim(),
        )
        .unwrap();
    expect(
        &actual,
        json!([
            {"type": "paragraph_open"},
            {"type": "inline", "children": [{"type": "text", "content": "foo <!-- example"}]},
            {"type": "paragraph_close"},
            {"type": "paragraph_open"},
            {"type": "inline", "content": "comment --> bar"},
            {"type": "paragraph_close"},
        ]),
    );
}

// --- frontmatter.test.ts --------------------------------------------------------------------

#[test]
fn parsing_frontmatter() {
    let example = parse(
        "
    ---
    test: true
    ---

    # Test
    ",
    );
    assert_eq!(example.len(), 4);
    expect(
        &example[..1],
        json!([{"type": "frontmatter", "content": "test: true", "level": 0, "map": [0, 2]}]),
    );
}

#[test]
fn parsing_frontmatter_only() {
    let example = parse(
        "
    ---
    test: true
    foo: bar
    ---
    ",
    );
    assert_eq!(example.len(), 1);
    expect(
        &example,
        json!([{"type": "frontmatter", "level": 0, "map": [0, 3]}]),
    );
}

#[test]
fn frontmatter_spaces_at_end() {
    let example = parse("\n    ---   \n    test: true\n    ---   \n\n    # Test\n    ");
    assert_eq!(example.len(), 4);
    expect(
        &example[..1],
        json!([{"type": "frontmatter", "content": "test: true", "level": 0, "map": [0, 2]}]),
    );
}

#[test]
fn frontmatter_missing_closing() {
    let example = parse(
        "
    ---
    test: true

    # Test
    ",
    );
    assert_eq!(example.len(), 7);
    assert_eq!(example[0].kind, "hr");
}

#[test]
fn frontmatter_missing_opening() {
    let example = parse(
        "
    test: true
    ---

    # Test
    ",
    );
    assert_eq!(example.len(), 7);
    assert_eq!(example[0].kind, "paragraph_open");
}

// --- link.test.ts -----------------------------------------------------------------------------

fn with_link_validation() -> Tokenizer {
    tokenizer(TokenizerOptions {
        allow_link_validation: true,
        ..TokenizerOptions::default()
    })
}

fn inline_error(tokens: &[Token]) -> Option<Value> {
    tokens
        .iter()
        .find(|t| t.kind == "inline")?
        .errors
        .as_ref()?
        .first()
        .cloned()
}

fn href_error() -> Value {
    json!({
        "id": "href-format-invalid",
        "level": "error",
        "message": "The 'href' format cannot contain Markdoc tag or variable. URLs must be static strings.",
    })
}

#[test]
fn accepts_valid_link_urls_and_markdoc_tag_in_one_paragraph() {
    let tokens = parse_with(
        &with_link_validation(),
        "The link is https://example.com. {% tag /%})",
    );
    assert_eq!(inline_error(&tokens), None);
}

#[test]
fn accepts_raw_tag_content_in_markdown_link_format() {
    let tokens = parse_with(&with_link_validation(), "[Link]({% tag %})");
    assert_eq!(inline_error(&tokens), None);
}

#[test]
fn rejects_tags_after_urls() {
    for example in [
        "https://example.com/{% tag %}content{% /tag %})",
        "https://en.wikipedia.org/wiki/Exam_(disambiguation){% tag /%}",
        "[Link](https://{% $variable.custom_value %})",
        "[Link](https://example.com/{% tag /%})",
        "[Link](https://example.com/{% tag %}content{% /tag %})",
    ] {
        let tokens = parse_with(&with_link_validation(), example);
        assert_eq!(inline_error(&tokens), Some(href_error()), "{example}");
    }
}

#[test]
fn rejects_custom_protocols_defined_in_the_config_with_markdoc_variable() {
    let t = tokenizer(TokenizerOptions {
        allow_link_validation: true,
        link_validation_options: Some(LinkPluginOptions::new(["vscode"])),
        ..TokenizerOptions::default()
    });
    let tokens = parse_with(&t, "[Link](vscode://{% $variable.custom_value %})");
    assert_eq!(inline_error(&tokens), Some(href_error()));
    // the default protocols are then not validated
    let tokens = parse_with(&t, "[Link](https://{% $variable.custom_value %})");
    assert_eq!(inline_error(&tokens), None);
}

// --- beyond upstream's tests ------------------------------------------------------------------

#[test]
fn options_default_as_upstream() {
    let t = Tokenizer::default();
    let md = t.parser();
    assert!(!md.options.allow_indentation);
    let enabled = |names: Vec<(&str, bool)>| {
        names
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(n, _)| n.to_owned())
            .collect::<Vec<_>>()
    };
    let block = enabled(
        md.block
            .ruler
            .rules()
            .iter()
            .map(|r| (r.name.as_str(), r.enabled))
            .collect(),
    );
    assert_eq!(
        block,
        [
            "table",
            "fence",
            "blockquote",
            "frontmatter",
            "hr",
            "list",
            "reference",
            "html_block",
            "heading",
            "annotations",
            "paragraph"
        ]
    );
    let inline: Vec<&str> = md
        .inline
        .ruler
        .rules()
        .iter()
        .map(|r| r.name.as_str())
        .collect();
    // node: `new Markdoc.Tokenizer().parser.inline.ruler.__rules__` (and ruler2's), names
    assert_eq!(
        inline,
        [
            "text",
            "linkify",
            "newline",
            "escape",
            "backticks",
            "strikethrough",
            "emphasis",
            "link",
            "image",
            "autolink",
            "html_inline",
            "entity",
            "containers"
        ]
    );
    let inline2: Vec<&str> = md
        .inline
        .ruler2
        .rules()
        .iter()
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(
        inline2,
        [
            "balance_pairs",
            "strikethrough",
            "emphasis",
            "fragments_join"
        ]
    );
    let core: Vec<&str> = md
        .core
        .ruler
        .rules()
        .iter()
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(core.last(), Some(&"annotations"));

    let all = Tokenizer::new(TokenizerOptions {
        allow_comments: true,
        allow_link_validation: true,
        ..TokenizerOptions::default()
    });
    let md = all.parser();
    assert_eq!(md.block.ruler.rules()[0].name, "comment");
    assert_eq!(md.block.ruler.rules()[0].alt, ["paragraph"]);
    assert_eq!(
        md.inline.ruler.rules().last().map(|r| r.name.as_str()),
        Some("comment")
    );
    assert_eq!(
        md.core.ruler.rules().last().map(|r| r.name.as_str()),
        Some("link_url_validation")
    );
}

#[test]
fn options_read_from_json_as_upstream_reads_its_config() {
    let config = json!({
        "allowIndentation": 1,
        "allowComments": "",
        "allowLinkValidation": true,
        "linkValidationOptions": null,
        "typographer": true,
    });
    let options = TokenizerOptions::from_json(config.as_object().unwrap()).unwrap();
    assert!(options.allow_indentation && !options.allow_comments);
    assert!(options.allow_link_validation && options.link_validation_options.is_none());
    assert_eq!(options.markdown_it.get("typographer"), Some(&json!(true)));
    let t = Tokenizer::new(options);
    assert!(t.parser().options.typographer && t.parser().options.allow_indentation);
    // any JSON is a config: linkValidationOptions is read as the JS value it is
    let odd = json!({"linkValidationOptions": {"validatedProtocols": "http"}});
    assert_eq!(
        TokenizerOptions::from_json(odd.as_object().unwrap())
            .unwrap()
            .link_validation_options,
        Some(LinkPluginOptions::new(["h", "t", "t", "p"]))
    );
}

/// What `tokenize` throws, and that the environment keeps no trace of it afterwards.
fn throws(t: &Tokenizer, src: &str) -> TokenizeError {
    t.tokenize(src).expect_err("upstream throws")
}

fn range_error() -> TokenizeError {
    TokenizeError {
        name: "RangeError".into(),
        message: "Maximum call stack size exceeded".into(),
    }
}

/// How many arrays nested in a tag at the top of a document fit: node's limit where the
/// stack grows (the arrays' cost on the depth budget), MAX_NESTING's where it cannot.
fn arrays_that_fit() -> usize {
    if crate::depth::can_grow() {
        (crate::depth::STACK / crate::grammar::cost::ARRAY) as usize
    } else {
        MAX_NESTING - 1
    }
}

#[test]
fn values_nested_too_deep_throw_a_range_error_wherever_the_tag_is() {
    // Too deep everywhere, and (ten levels less, for the blockquote around one) deep enough
    // to fit everywhere.
    let (too_deep, fitting) = if crate::depth::can_grow() {
        (arrays_that_fit() + 1, arrays_that_fit() - 10)
    } else {
        (MAX_NESTING, MAX_NESTING - 1)
    };
    let deep = format!("x={}1{}", "[".repeat(too_deep), "]".repeat(too_deep));
    let fits = format!("x={}1{}", "[".repeat(fitting), "]".repeat(fitting));
    let t = Tokenizer::default();
    // node: each of these throws RangeError: Maximum call stack size exceeded (with V8's
    // deeper limit)
    for (where_, src) in [
        ("block", format!("{{% tag {deep} %}}\n")),
        ("inline", format!("a {{% tag {deep} %}}\n")),
        ("fence info", format!("```js {{% {deep} %}}\nx\n```\n")),
        ("fence content", format!("```\n{{% tag {deep} %}}\n```\n")),
        (
            "after an error",
            format!("{{% 1 %}}\n\n> b {{% tag {deep} /%}}"),
        ),
    ] {
        assert_eq!(throws(&t, &src), range_error(), "{where_}");
        let fine = src.replace(&deep, &fits);
        assert!(t.tokenize(&fine).is_ok(), "{where_}: less deep parses");
    }
    // a process=false fence's content is not parsed for tags
    let skipped = format!("``` {{% process=false %}}\n{{% tag {deep} %}}\n```\n");
    assert!(t.tokenize(&skipped).is_ok());
    assert_eq!(
        range_error().to_string(),
        "RangeError: Maximum call stack size exceeded"
    );
}

#[test]
fn a_comment_in_a_link_label_throws_as_skip_token_does() {
    // node (allowComments): Error: inline rule didn't increment state.pos; without the
    // comments plugin the label is text.
    let error = throws(&with_comments(), "a [<!-- x -->](y) b");
    assert_eq!(
        error.to_string(),
        "Error: inline rule didn't increment state.pos"
    );
    assert!(Tokenizer::default().tokenize("[<!-- x -->](y)").is_ok());
    // not in a label: a comment token
    assert!(with_comments().tokenize("a <!-- x --> [y](z)").is_ok());
}

#[test]
fn a_closing_tag_in_a_link_throws_the_type_error_upstream_throws() {
    // node: the TypeError each throws; a later tag restores the delimiter list, and a tag
    // open around the link leaves one to pop
    let t = Tokenizer::default();
    let type_error = |property: &str| TokenizeError {
        name: "TypeError".into(),
        message: format!("Cannot read properties of undefined (reading '{property}')"),
    };
    for (src, property) in [
        ("[{%/a%}]()", "length"),
        ("[a {% /b %}](u) ~c", "length"),
        ("[a {% /b %}](u) *c*", "push"),
        ("[a {% /b %}](u) _c_", "push"),
        ("[a {% /b %}](u) ~~c~~", "push"),
        ("[r]: /u\n\n[{% /a %}][r]", "length"),
    ] {
        assert_eq!(throws(&t, src), type_error(property), "{src}");
    }
    for src in [
        "[a {% /b %}](u) {% x /%} *c*",
        "{% a %}[x {% /b %}](u) *c*",
        "![a {% /b %}](u) *c*",
    ] {
        assert!(t.tokenize(src).is_ok(), "{src}");
    }
    // the first throw is the one that escapes: the comment's, before the TypeError
    assert_eq!(
        throws(&with_comments(), "[{%/a%}]()[<!-->").message,
        "inline rule didn't increment state.pos"
    );
}

#[test]
fn plugins_on_a_markdown_it_of_ones_own_record_the_throw_on_the_environment() {
    let mut md = crate::markdown_it::MarkdownIt::default();
    super::plugins::annotations::plugin(&mut md).unwrap();
    let mut env = Env::default();
    let deep = format!(
        "{{% a x={}1{} %}}",
        "[".repeat(arrays_that_fit() + 1),
        "]".repeat(arrays_that_fit() + 1)
    );
    md.parse(&deep, &mut env);
    assert_eq!(
        env.thrown,
        Some(crate::markdown_it::JsError::stack_overflow())
    );
}

#[test]
fn the_deepest_tag_fits_a_two_mib_thread() {
    // The tokenizer at its deepest (a tag inside 100 nested images, markdown-it's default
    // maxNesting) calling the grammar at its deepest (`f(1, ...)` takes the most stack per
    // level), on Rust's default thread stack: no overflow, in debug builds too. Where the
    // stack grows, the images and the calls share node's stack (the depth budget).
    let levels = if crate::depth::can_grow() {
        use crate::grammar::cost::{CALL, PARAMETER_TAIL};
        let images = 100 * crate::markdown_it::cost::IMAGE;
        ((crate::depth::STACK - images) / (CALL + PARAMETER_TAIL)) as usize + 1
    } else {
        MAX_NESTING
    };
    let outcome = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let t = Tokenizer::new(TokenizerOptions {
                allow_indentation: true,
                allow_comments: true,
                ..TokenizerOptions::default()
            });
            let tag = |levels: usize| {
                let d = levels - 1;
                format!("{{% tag x={}1{} /%}}", "f(1, ".repeat(d), ")".repeat(d))
            };
            let doc = |levels: usize| {
                format!(
                    "{}{}{}\n",
                    "![".repeat(100),
                    tag(levels),
                    "](u)".repeat(100)
                )
            };
            let tokens = t.tokenize(&doc(levels)).expect("at the limit");
            // the tag was parsed, inside the innermost image (node: 100 images deep)
            let mut t0 = &tokens[1].children.as_ref().unwrap()[0];
            let mut depth = 0;
            while t0.kind == "image" {
                depth += 1;
                t0 = &t0.children.as_ref().unwrap()[0];
            }
            assert_eq!((depth, t0.kind.as_str()), (100, "tag"));
            assert_eq!(t.tokenize(&doc(levels + 1)).unwrap_err(), range_error());
            let blocks = format!("{}{}\n", "> ".repeat(99), tag(MAX_NESTING));
            assert!(t.tokenize(&blocks).is_ok());
        })
        .unwrap()
        .join();
    assert!(outcome.is_ok(), "{levels}");
}

#[test]
fn a_tokenizer_can_be_shared_between_threads() {
    // as upstream's index.ts keeps one default `tokenizer` for every `parse(string)`
    fn shared<T: Send + Sync>() {}
    shared::<Tokenizer>();
    static DEFAULT: std::sync::OnceLock<Tokenizer> = std::sync::OnceLock::new();
    let t = DEFAULT.get_or_init(Tokenizer::default);
    let kinds: Vec<String> = std::thread::scope(|s| {
        let h = s.spawn(|| t.tokenize("{% a /%}\n").unwrap());
        h.join().unwrap().into_iter().map(|t| t.kind).collect()
    });
    assert_eq!(kinds, ["tag"]);
}

#[test]
fn link_validation_options_are_read_as_the_js_value_they_are() {
    // node, upstream's dist: new Tokenizer({allowLinkValidation: true, linkValidationOptions:
    // O}).tokenize(D): "ERR" where an inline token got href-format-invalid, "ok" where none
    // did, or what it threw (`T`: TypeError: allowedProtocols is not iterable)
    let docs = [
        "x https://a{% b %}",
        "plain",
        "s://a{% b %}",
        "1://x{% y %}",
        "null://x{% y %}",
        "1,2://x{% y %}",
        "😀://x{% y %}",
        "://x{% y %}",
        "x {% a",
        "{% a %} https://x{% b %}",
    ];
    let not_iterable = ["T", "ok", "T", "T", "T", "T", "T", "T", "ok", "T"];
    let cases: [(Value, [&str; 10]); 14] = [
        (
            Value::Null,
            ["ERR", "ok", "ok", "ok", "ok", "ok", "ok", "ok", "ok", "ok"],
        ),
        (json!({}), not_iterable),
        (json!(false), not_iterable),
        (json!(0), not_iterable),
        (json!("x"), not_iterable),
        (json!([]), not_iterable),
        (
            json!({"validatedProtocols": "https"}),
            ["ERR", "ok", "ERR", "ok", "ok", "ok", "ok", "ok", "ok", "ok"],
        ),
        (
            json!({"validatedProtocols": [1]}),
            ["ok", "ok", "ok", "ERR", "ok", "ok", "ok", "ok", "ok", "ok"],
        ),
        (
            json!({"validatedProtocols": [null]}),
            ["ok", "ok", "ok", "ok", "ERR", "ok", "ok", "ok", "ok", "ok"],
        ),
        (
            json!({"validatedProtocols": [[1, 2]]}),
            ["ok", "ok", "ok", "ok", "ok", "ERR", "ok", "ok", "ok", "ok"],
        ),
        (json!({"validatedProtocols": 5}), not_iterable),
        (json!({"validatedProtocols": null}), not_iterable),
        (
            json!({"validatedProtocols": "😀s"}),
            [
                "ERR", "ok", "ERR", "ok", "ok", "ok", "ERR", "ok", "ok", "ok",
            ],
        ),
        (
            json!({"validatedProtocols": [""]}),
            [
                "ERR", "ok", "ERR", "ERR", "ERR", "ERR", "ERR", "ERR", "ok", "ok",
            ],
        ),
    ];
    for (options, expected) in cases {
        let mut config = json!({"allowLinkValidation": true});
        if !options.is_null() {
            config["linkValidationOptions"] = options.clone();
        }
        let t = Tokenizer::new(TokenizerOptions::from_json(config.as_object().unwrap()).unwrap());
        let got: Vec<&str> = docs
            .iter()
            .map(|d| match t.tokenize(d) {
                Ok(tokens) if tokens.iter().any(|t| t.errors.is_some()) => "ERR",
                Ok(_) => "ok",
                Err(e) => {
                    assert_eq!(e.to_string(), "TypeError: allowedProtocols is not iterable");
                    "T"
                }
            })
            .collect();
        assert_eq!(got, expected, "{options}");
    }
    // with link validation off they are never read
    let off = json!({"linkValidationOptions": 5});
    let t = Tokenizer::new(TokenizerOptions::from_json(off.as_object().unwrap()).unwrap());
    assert!(t.tokenize("x https://a{% b %}").is_ok());
}

#[test]
fn allow_indentation_can_come_from_a_proto_object() {
    // node: Object.assign(md.options, config) sets md.options' prototype to config.__proto__
    // (an own key after JSON.parse); `md.options.allowIndentation` is then inherited
    let indented = "    indented para\n\n    - item\n";
    let kinds = |config: Value| -> Vec<String> {
        let t = Tokenizer::new(TokenizerOptions::from_json(config.as_object().unwrap()).unwrap());
        t.tokenize(indented)
            .unwrap()
            .into_iter()
            .map(|t| t.kind)
            .collect()
    };
    let list = [
        "paragraph_open",
        "inline",
        "paragraph_close",
        "bullet_list_open",
    ];
    let inherited = kinds(json!({"__proto__": {"allowIndentation": true}}));
    assert_eq!(inherited[..4], list);
    // an own allowIndentation wins; other values of __proto__ change nothing
    let own = kinds(json!({"allowIndentation": false, "__proto__": {"allowIndentation": true}}));
    assert_eq!(own, kinds(json!({})));
    assert_eq!(kinds(json!({"__proto__": null})), kinds(json!({})));
    assert_eq!(kinds(json!({"__proto__": 5})), kinds(json!({})));
}

/// `tests/next/tokenizer/linkify_golden.jsonl` (written by linkify_golden.py from upstream's
/// dist): token streams with `linkify: true`, under three configurations.
#[test]
fn linkify_tokenizes_as_upstream() {
    use crate::next::compare::first_difference;
    let path = format!(
        "{}/tests/next/tokenizer/linkify_golden.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).expect("linkify_golden.jsonl");
    let mut checked = 0;
    let mut failures = Vec::new();
    for line in text.split('\n').filter(|l| !l.trim().is_empty()) {
        let case: Value = serde_json::from_str(line).expect("a case");
        let id = case["id"].as_str().unwrap_or("?");
        let config = case["tokenizer"].as_object().cloned().unwrap_or_default();
        let t = Tokenizer::new(TokenizerOptions::from_json(&config).unwrap());
        let failure = match (
            t.tokenize(case["code"].as_str().unwrap()),
            case.get("tokens"),
        ) {
            (Ok(tokens), Some(expected)) => first_difference(expected, &tokens_json(&tokens))
                .map(|(path, a, b)| format!("{id}: {path}: upstream {a} | port {b}")),
            (Err(e), None) => {
                let threw = &case["threw"];
                (threw["name"] != e.name.as_str() || threw["message"] != e.message.as_str())
                    .then(|| format!("{id}: upstream threw {threw}, the port {e}"))
            }
            (outcome, _) => Some(format!("{id}: {:?}", outcome.map(|t| t.len()))),
        };
        failures.extend(failure);
        checked += 1;
    }
    assert!(checked >= 100, "{checked} cases");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_grammar_error_quoting_half_an_astral_character_has_u_fffd() {
    // node: {"type":"error",...,"meta":{"error":{"message":"Expected \"/\", class, id,
    // identifier, tag name, or variable but \"\ud83d\" found.", ...}}}: half of the emoji,
    // which the port's JSON string holds as U+FFFD
    let tokens = parse("{% \u{1F600} %}");
    let message = tokens_json(&tokens)[0]["meta"]["error"]["message"].clone();
    assert_eq!(
        message,
        json!(
            "Expected \"/\", class, id, identifier, tag name, or variable but \"\u{FFFD}\" found."
        )
    );
}
