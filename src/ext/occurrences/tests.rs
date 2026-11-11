//! Against the real tokenizer with D3's options (`allowIndentation` and `allowComments` on),
//! and with them off where they change what is a tag.
use super::*;
use crate::ext::parse;
use crate::types::NodeType;

/// D3's options.
fn d3() -> ParseOptions {
    ParseOptions {
        tokenizer: TokenizerOptions {
            allow_indentation: true,
            allow_comments: true,
            ..TokenizerOptions::default()
        },
        ..ParseOptions::default()
    }
}

fn find(src: &str) -> Vec<Occurrence> {
    occurrences(src, &d3()).expect("tokenizes")
}

/// `kind name text [partner] [fence]` per occurrence.
fn describe_with(options: &ParseOptions, src: &str) -> Vec<String> {
    let found = occurrences(src, options).expect("tokenizes");
    found
        .iter()
        .map(|o| {
            let kind = match &o.kind {
                OccurrenceKind::Open { .. } => "open",
                OccurrenceKind::SelfClosing { .. } => "self",
                OccurrenceKind::Close { .. } => "close",
                OccurrenceKind::Annotation { .. } => "annotation",
                OccurrenceKind::Interpolation { .. } => "interpolation",
                OccurrenceKind::Error { .. } => "error",
            };
            let mut line = format!("{kind} {}", &src[o.span.clone()]);
            if let Some(p) = o.partner {
                line += &format!(" ~{p}");
            }
            if o.in_fence {
                line += " fence";
            }
            line
        })
        .collect()
}

fn describe(src: &str) -> Vec<String> {
    describe_with(&d3(), src)
}

#[test]
fn every_kind_with_its_span() {
    let src = "# Title {% #intro .lead %}\n\n{% callout type=\"note\" %}\nText {% $user.name %} and {% upper(\"a\") %}.\n{% /callout %}\n\n{% image src=\"a.png\" /%}\n\n{% bad , %}\n\n{% bad \"unterminated %}\n";
    assert_eq!(
        describe(src),
        [
            "annotation {% #intro .lead %}",
            "open {% callout type=\"note\" %} ~4",
            "interpolation {% $user.name %}",
            "interpolation {% upper(\"a\") %}",
            "close {% /callout %} ~1",
            "self {% image src=\"a.png\" /%}",
            // An unterminated string hides the `%}`: no tag at all.
            "error {% bad , %}",
        ]
    );
    let found = find(src);
    let OccurrenceKind::Annotation { attributes } = &found[0].kind else {
        panic!("an annotation")
    };
    assert_eq!(attributes.get("id"), Some(&Scalar::String("intro".into())));
    assert_eq!(
        attributes.get("class"),
        Some(&Scalar::Object(IndexMap::from([(
            "lead".to_string(),
            Scalar::Boolean(true)
        )])))
    );
    let OccurrenceKind::Interpolation { value } = &found[2].kind else {
        panic!("an interpolation")
    };
    assert!(matches!(value, Scalar::Variable(v) if v.path.len() == 2));
    let OccurrenceKind::Interpolation { value } = &found[3].kind else {
        panic!("an interpolation")
    };
    assert!(matches!(value, Scalar::Function(f) if f.name == "upper"));
    let OccurrenceKind::Error { message } = &found[6].kind else {
        panic!("an error")
    };
    assert!(!message.is_empty());
    // Spans: 1-based lines, UTF-16 columns, byte offsets.
    assert_eq!(found[2].location.start.line, 4);
    assert_eq!(found[2].location.start.character, Some(5));
    assert_eq!(found[2].location.start.offset, found[2].span.start);
    assert_eq!(found[4].location.end.offset, found[4].span.end);
}

#[test]
fn attributes_are_as_written() {
    let found = find("{% x \"p\" a=1 b=$v c=[1, {d: f($w)}] .k /%}\n");
    let OccurrenceKind::SelfClosing { name, attributes } = &found[0].kind else {
        panic!("self-closing")
    };
    assert_eq!(name, "x");
    assert_eq!(
        attributes.keys().collect::<Vec<_>>(),
        ["primary", "a", "b", "c", "class"]
    );
    assert_eq!(attributes["primary"], Scalar::String("p".into()));
    assert_eq!(attributes["a"], Scalar::Number(1.0));
    assert!(matches!(&attributes["b"], Scalar::Variable(_)));
    let Scalar::Array(items) = &attributes["c"] else {
        panic!("an array")
    };
    let Scalar::Object(o) = &items[1] else {
        panic!("an object")
    };
    assert!(matches!(&o["d"], Scalar::Function(f) if f.name == "f"));
}

#[test]
fn self_closing_and_open_close_are_told_apart() {
    assert_eq!(
        describe("{% x /%}\n\n{% x %}{% /x %}\n"),
        ["self {% x /%}", "open {% x %} ~2", "close {% /x %} ~1"]
    );
}

#[test]
fn pairing_is_the_parsers() {
    // A closing tag closes the top of the stack only.
    assert_eq!(
        describe("{% a %}\n{% b %}\n{% /a %}\n"),
        ["open {% a %}", "open {% b %}", "close {% /a %}"]
    );
    assert_eq!(
        describe("{% a %}\n\n- {% b %}x{% /b %}\n\n{% /a %}\n"),
        [
            "open {% a %} ~3",
            "open {% b %} ~2",
            "close {% /b %} ~1",
            "close {% /a %} ~0"
        ]
    );
    // An inline tag left open is never closed by a block closing tag.
    assert_eq!(
        describe("p {% x %} q\n\n{% /x %}\n"),
        ["open {% x %}", "close {% /x %}"]
    );
    // Its own closing tag on a later line of the same paragraph closes it.
    assert_eq!(
        describe("p {% x %} q\nr {% /x %}\n"),
        ["open {% x %} ~1", "close {% /x %} ~0"]
    );
}

#[test]
fn what_is_not_a_tag_is_not_an_occurrence() {
    let src = "`{% code %}` \\{% escaped %}\n\n![{% alt %}](a.png)\n\n```js {% process=false %}\n{% raw %}\n```\n";
    // Only the fence's own tag: `process=false` keeps its content text.
    assert_eq!(describe(src), ["annotation {% process=false %}"]);
}

#[test]
fn a_tag_in_a_fence_info_string_is_the_fences_annotation() {
    let src = "```js {% .lang #f process=true %}\nx {% a /%}\n```\n\n~~~ {% bad , %}\n~~~\n";
    assert_eq!(
        describe(src),
        [
            "annotation {% .lang #f process=true %}",
            "self {% a /%} fence",
            "error {% bad , %}"
        ]
    );
    let found = find(src);
    let OccurrenceKind::Annotation { attributes } = &found[0].kind else {
        panic!("an annotation")
    };
    assert_eq!(
        attributes.keys().collect::<Vec<_>>(),
        ["class", "id", "process"]
    );
    assert!(!found[0].in_fence);
    assert_eq!(found[0].location.start.character, Some(6));
}

#[test]
fn tags_in_a_fence_are_occurrences_in_the_fence() {
    let src = "```\nx {% a %}y{% /a %}\n```\n\n{% b /%}\n";
    assert_eq!(
        describe(src),
        [
            "open {% a %} ~1 fence",
            "close {% /a %} ~0 fence",
            "self {% b /%}"
        ]
    );
}

#[test]
fn the_tokenizer_switches_decide() {
    let src = "<!-- {% hidden %} -->\n\n    {% indented /%}\n";
    // D3's options: the comment hides its tag; the indented tag is a block tag.
    assert_eq!(describe(src), ["self {% indented /%}"]);
    // Upstream's defaults: the comment is text, so its tag is one.
    let plain = ParseOptions::default();
    assert_eq!(
        describe_with(&plain, src),
        ["open {% hidden %}", "self {% indented /%}"]
    );
}

#[test]
fn tags_in_tables_and_lists_are_placed() {
    let src = "| a | b |\n| - | - |\n| {% x /%} | y {% $v %} |\n\n1. one {% .c %}\n   - two {% t \"z\" /%}\n";
    assert_eq!(
        describe(src),
        [
            "self {% x /%}",
            "interpolation {% $v %}",
            "annotation {% .c %}",
            "self {% t \"z\" /%}"
        ]
    );
}

#[test]
fn spans_point_into_the_text_given_with_a_byte_order_mark_stripped() {
    let src = "\u{FEFF}{% a %}\n# Héllo {% b /%}\n{% /a %}\n";
    let options = ParseOptions {
        strip_bom: true,
        args: ParserArgs::file("f.md"),
        ..d3()
    };
    let found = occurrences(src, &options).unwrap();
    let texts: Vec<&str> = found.iter().map(|o| &src[o.span.clone()]).collect();
    assert_eq!(texts, ["{% a %}", "{% b /%}", "{% /a %}"]);
    assert_eq!(found[0].partner, Some(2));
    assert_eq!(found[1].location.start.line, 2);
    // UTF-16 columns: "# Héllo " is 8 units.
    assert_eq!(found[1].location.start.character, Some(8));
    assert_eq!(found[1].location.file.as_deref(), Some("f.md"));
}

#[test]
fn the_ast_is_the_parsers_and_agrees_with_the_occurrences() {
    let src = "# T {% .a %}\n\n{% note %}\nSee {% ref \"x\" /%} and {% $v %}.\n\n- item {% i %}inline{% /i %}\n{% /note %}\n\n{% open %}\nunclosed\n";
    // With this crate's spans on, so the tag nodes can be compared with the occurrences.
    let options = d3().with_spans(true);
    let (node, found) = parse_with_occurrences(src, &options).unwrap();
    assert_eq!(
        format!("{node:?}"),
        format!("{:?}", parse(src, &options).unwrap())
    );
    // Every tag node the parser made starts where an opening or self-closing occurrence does.
    let starts: Vec<usize> = found
        .iter()
        .filter(|o| {
            matches!(
                o.kind,
                OccurrenceKind::Open { .. } | OccurrenceKind::SelfClosing { .. }
            )
        })
        .map(|o| o.span.start)
        .collect();
    let tags: Vec<usize> = node
        .walk()
        .filter(|n| n.node_type == NodeType::Tag)
        .map(|n| n.span.as_ref().unwrap().start.offset)
        .collect();
    assert_eq!(tags, starts);
    // And the one the parser reports `missing-closing` for has no partner.
    let open = found
        .iter()
        .find(|o| matches!(&o.kind, OccurrenceKind::Open { name, .. } if name == "open"))
        .unwrap();
    assert_eq!(open.partner, None);
    assert!(
        node.walk()
            .any(|n| n.errors.iter().any(|e| e.id == "missing-closing"))
    );
}

#[test]
fn an_error_tokenizing_is_an_err() {
    // The tokenizer throws for a closing tag inside a link's text, as upstream does.
    assert!(occurrences("[{% /a %}](u)", &d3()).is_err());
}
