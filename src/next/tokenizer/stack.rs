//! The stack the tokenizer needs, and the engine's recursion limits: every construct that
//! recurses, at the limit and one past it, on a thread with 2 MiB of stack.
//!
//! Where the stack grows ([`crate::depth::can_grow`]), each construct nests as deep as node's
//! stack takes it ([`crate::markdown_it::cost`] on the depth budget), alone, in pairs, and
//! with a tag's values inside.
//!
//! Where it cannot, the engine stops at [`crate::markdown_it::MAX_DEPTH`] nested calls and the
//! grammar at [`crate::grammar::MAX_NESTING`]. Measured in a debug build (`probe`, by a script
//! that halves the thread's stack until it overflows; 4 KiB steps): the grammar at its
//! deepest needs 1,167 KiB with the tokenizer around it; each level of nesting adds 2.2 KiB
//! for a list item, 1.8 for a blockquote, 2.0 for an image, 1.1 for an unclosed `[`. At the
//! limit (159 nested lists around the deepest tag) the thread needs 1,523 KiB of its 2,048: a
//! quarter is left. Release builds need a fraction of that.

use super::{TokenizeError, Tokenizer, TokenizerOptions};
use crate::grammar::MAX_NESTING;

/// The deepest tag value the grammar takes (`f(1, ` takes the most stack per level).
fn deepest_tag() -> String {
    let d = MAX_NESTING - 1;
    format!("{{% tag x={}1{} /%}}", "f(1, ".repeat(d), ")".repeat(d))
}

/// A document nesting `construct` `depth` times around `inner`.
pub(super) fn nested(construct: &str, depth: usize, inner: &str) -> String {
    match construct {
        "blockquote" => format!("{}{inner}\n", "> ".repeat(depth)),
        "list" => format!("{}{inner}\n", "- ".repeat(depth)),
        "ordered" => format!("{}{inner}\n", "1. ".repeat(depth)),
        "quoted-list" => format!("{}{inner}\n", "> - ".repeat(depth)),
        "image" => format!("{}{inner}{}\n", "![".repeat(depth), "](u)".repeat(depth)),
        "link-image" => format!(
            "{}{inner}{}\n",
            "[![".repeat(depth),
            "](u)](v)".repeat(depth)
        ),
        "bracket" => format!("{}{inner}\n", "[".repeat(depth)),
        other => panic!("unknown construct {other}"),
    }
}

/// A Tokenizer with both of Markdoc's switches on and markdown-it's `maxNesting` raised far
/// past what any input reaches.
pub(super) fn unlimited() -> Tokenizer {
    let mut markdown_it = serde_json::Map::new();
    markdown_it.insert("maxNesting".into(), 1e9.into());
    Tokenizer::new(TokenizerOptions {
        allow_indentation: true,
        allow_comments: true,
        markdown_it,
        ..TokenizerOptions::default()
    })
}

/// `tokenize` on a thread with `stack` bytes of stack.
fn on_stack(stack: usize, doc: String) -> Result<usize, TokenizeError> {
    std::thread::Builder::new()
        .stack_size(stack)
        .spawn(move || unlimited().tokenize(&doc).map(|t| t.len()))
        .expect("spawn")
        .join()
        .expect("no panic")
}

/// Measuring tool, run by hand (each overflow aborts the process, so a script runs it once
/// per stack size): tokenizes `nested($PROBE_CONSTRUCT, $PROBE_DEPTH, tag or "x")` on a
/// thread with `$PROBE_STACK` bytes and prints the outcome.
#[test]
#[ignore = "a measuring tool; see the module docs"]
fn probe() {
    let var = |k: &str| std::env::var(k).ok();
    let (Some(construct), Some(depth), Some(stack)) = (
        var("PROBE_CONSTRUCT"),
        var("PROBE_DEPTH"),
        var("PROBE_STACK"),
    ) else {
        return;
    };
    let inner = if var("PROBE_TAG").as_deref() == Some("1") {
        deepest_tag()
    } else {
        "x".to_owned()
    };
    let doc = nested(&construct, depth.parse().unwrap(), &inner);
    let outcome = on_stack(stack.parse().unwrap(), doc);
    println!("PROBE {construct} {depth} {stack} {outcome:?}");
}

/// Measuring tool, run by hand: the deepest nesting of each construct that tokenizes without
/// the engine's RangeError, with an 8 MiB stack (release builds give the same depths).
#[test]
#[ignore = "a measuring tool"]
fn deepest_nesting() {
    for construct in [
        "blockquote",
        "list",
        "ordered",
        "quoted-list",
        "image",
        "link-image",
        "bracket",
    ] {
        let fits = |n: usize| on_stack(8 << 20, nested(construct, n, "x")).is_ok();
        let (mut lo, mut hi) = (1, 2000);
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if fits(mid) { lo = mid } else { hi = mid }
        }
        println!("DEEPEST {construct} {lo}");
    }
}

/// `(construct, the deepest nesting the port takes where the stack cannot grow)`: past it,
/// `MAX_DEPTH` (160) nested calls of the engine's recursive parsers would be needed.
const LIMITS: [(&str, usize); 7] = [
    ("blockquote", 159),
    ("list", 159),
    ("ordered", 159),
    ("quoted-list", 79),
    ("image", 159),
    ("link-image", 79),
    ("bracket", 159),
];

/// What a level of `construct` costs on the depth budget (a pair for the mixed ones).
fn level_cost(construct: &str) -> u64 {
    use crate::markdown_it::cost::*;
    match construct {
        "blockquote" => BLOCKQUOTE,
        "list" | "ordered" => LIST,
        "quoted-list" => BLOCKQUOTE + LIST,
        "image" => IMAGE,
        "link-image" => LINK + IMAGE,
        "bracket" => LINK,
        other => panic!("unknown construct {other}"),
    }
}

/// The deepest nesting of `construct` the port takes around `x`: node's (fresh) where the
/// stack grows (1,789 blockquotes, 1,690 list items, 1,642 images, 1,604 unclosed `[`, and
/// as many pairs as both costs leave), else [`LIMITS`]'.
fn limit(construct: &str) -> usize {
    if crate::depth::can_grow() {
        (crate::depth::STACK / level_cost(construct)) as usize
    } else {
        LIMITS
            .iter()
            .find(|(c, _)| *c == construct)
            .expect("a construct")
            .1
    }
}

fn range_error() -> TokenizeError {
    TokenizeError {
        name: "RangeError".to_owned(),
        message: "Maximum call stack size exceeded".to_owned(),
    }
}

#[test]
fn every_construct_at_the_limit_fits_two_mib_and_one_more_level_is_a_range_error() {
    // Where the stack cannot grow, with the deepest tag inside (the most stack a debug build
    // needs); where it grows, the constructs alone, then with a tag's values inside.
    let inner = if crate::depth::can_grow() {
        "x".to_owned()
    } else {
        deepest_tag()
    };
    for (construct, _) in LIMITS {
        let limit = limit(construct);
        let at_limit = on_stack(2 << 20, nested(construct, limit, &inner));
        assert!(at_limit.is_ok(), "{construct} {limit}: {at_limit:?}");
        assert_eq!(
            on_stack(2 << 20, nested(construct, limit + 1, &inner)),
            Err(range_error()),
            "{construct} {}",
            limit + 1
        );
    }
    if crate::depth::can_grow() {
        // Half the stack in blockquotes, the other half in arrays in a tag inside them.
        use crate::depth::STACK;
        use crate::grammar::cost::ARRAY;
        let quotes = limit("blockquote") / 2;
        let left = STACK - quotes as u64 * crate::markdown_it::cost::BLOCKQUOTE;
        let arrays = (left / ARRAY) as usize;
        let tag = |n: usize| format!("{{% x a={}1{} /%}}", "[".repeat(n), "]".repeat(n));
        assert!(on_stack(2 << 20, nested("blockquote", quotes, &tag(arrays))).is_ok());
        assert_eq!(
            on_stack(2 << 20, nested("blockquote", quotes, &tag(arrays + 1))),
            Err(range_error())
        );
    }
}

#[test]
fn the_tag_at_the_limit_is_parsed() {
    // the deepest tag inside 159 blockquotes is a block tag token, 159 levels down
    let tokens = unlimited()
        .tokenize(&nested("blockquote", 159, &deepest_tag()))
        .unwrap();
    let tag = tokens.iter().find(|t| t.kind == "tag").expect("the tag");
    assert_eq!(tag.level, 159);
}

#[test]
fn far_past_the_limit_the_error_comes_at_once_and_memory_stays_small() {
    // the shapes that ran away before the limit existed (a list 1,100 deep then 1,100
    // blockquotes reached 30 GB): the RangeError, quickly, on a 2 MiB thread. (Where the
    // stack grows, each half is deeper than node takes it alone: an inline's images are
    // parsed after the blocks around them have returned, in markdown-it as here.)
    let n = if crate::depth::can_grow() { 2000 } else { 1100 };
    let deep = |a: &str, b: &str| format!("{}{}x\n", a.repeat(n), b.repeat(n));
    for doc in [
        deep("- ", "> "),
        deep("> ", "- "),
        deep("1. ", "![x"),
        nested("link-image", 5000, "x"),
        nested("bracket", 100_000, "x"),
    ] {
        assert_eq!(on_stack(2 << 20, doc), Err(range_error()));
    }
}

#[test]
fn markdown_its_own_max_nesting_still_stops_first() {
    // with the default maxNesting (100) nothing reaches MAX_DEPTH: deep input is cut off as
    // markdown-it cuts it, with no error
    let t = Tokenizer::new(TokenizerOptions {
        allow_indentation: true,
        allow_comments: true,
        ..TokenizerOptions::default()
    });
    for (construct, _) in LIMITS {
        let doc = nested(construct, 1100, "x");
        let outcome = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn({
                let t = t.clone();
                move || t.tokenize(&doc).map(|t| t.len())
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(outcome.is_ok(), "{construct}: {outcome:?}");
    }
}
