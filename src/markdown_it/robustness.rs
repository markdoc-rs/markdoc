//! The engine's public functions on arbitrary arguments: no argument makes one panic.
//!
//! [`every_public_function_takes_any_argument`] calls each public function of
//! `markdown_it` (and of `next::tokenizer`, which is built on it) with random and extreme
//! arguments under `catch_unwind`: texts with lone surrogates and every kind of character the
//! rules look for, positions and lines past the end, negative and huge numbers, names no
//! preset or rule has, options of every JSON shape, and documents parsed under them. A round
//! calls every function once; `MDIT_ROBUSTNESS=N` runs N rounds (`cargo test` runs a few
//! hundred), `MDIT_ROBUSTNESS_SEED` changes the seed, and `MDIT_ROBUSTNESS_TRACE` prints each
//! call before making it.
//!
//! Not called with arbitrary values: a parser state's fields set out of step with each other
//! (a mark past the source, `pos_max` past it), which the parsers keep consistent and the
//! rules take as such (see "Rules and parsers past their state" in the module docs). Line
//! numbers given to a block rule or to `ParserBlock::tokenize` are arguments, and are tried
//! past the state's end; so are plugin rules that push an `inline` token without children,
//! a closing token with nothing open, and delimiters pointing outside the token list.

use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::{Map, Value};

use super::common::{entities, html_re, ucmicro, utils};
use super::helpers::{parse_link_destination, parse_link_label, parse_link_title};
use super::linkify_it::rebuilder::{self, REBuilder, REBuilderOptions};
use super::linkify_it::{LinkifyIt, LinkifyOptions, Match};
use super::rules_block::state_block::StateBlock;
use super::rules_inline::state_inline::{Delimiter, StateInline};
use super::{
    AttrValue, BlockRule, Env, InlineRule, MarkdownIt, Options, Quotes, Token, js, mdurl, punycode,
};
use crate::next::tokenizer::{
    LinkPluginOptions, Tokenizer, TokenizerOptions, plugins, utils as tk,
};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

/// Code units the rules look for, and the ones JS strings hold that Rust strings do not.
const UNITS: &[u16] = &[
    0x00, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29,
    0x2A, 0x2B, 0x2C, 0x2D, 0x2E, 0x2F, 0x30, 0x31, 0x39, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E, 0x3F, 0x40,
    0x41, 0x46, 0x5A, 0x5B, 0x5C, 0x5D, 0x5E, 0x5F, 0x60, 0x61, 0x66, 0x68, 0x6E, 0x70, 0x74, 0x78,
    0x7A, 0x7B, 0x7C, 0x7D, 0x7E, 0x7F, 0x85, 0xA0, 0xAD, 0xE9, 0x130, 0x17F, 0x2028, 0x3000,
    0x3002, 0xFEFF, 0xFF0E, 0xFF5C, 0xFFFD, 0xFFFF, 0xD800, 0xD83D, 0xDBFF, 0xDC00, 0xDE00, 0xDFFF,
    0xD800, 0xDD00,
];

const PIECES: &[&str] = &[
    "http://",
    "https://",
    "mailto:",
    "//",
    "xn--",
    "xn--zc0c",
    "www.",
    ".com",
    "@",
    "[",
    "](",
    ")",
    "<",
    ">",
    "<a>",
    "</a>",
    "<!--",
    "-->",
    "{%",
    "%}",
    "{% a %}",
    "{% /a %}",
    "```",
    "~~~",
    "    ",
    "\t",
    "> ",
    "- ",
    "1. ",
    "# ",
    "---",
    "***",
    "| a |",
    "|---|",
    "&amp;",
    "&#x",
    "&#0;",
    "%",
    "%E2%82",
    "%ZZ",
    "\\",
    "\n\n",
    "\r\n",
    "*",
    "**",
    "_",
    "~~",
    "`",
    "``",
    "\"",
    "'",
    "(c)",
    "...",
    "--",
    "a",
    "b",
    "1",
    "9",
    "é",
    "😀",
    "𐄀",
    "[x]: ",
    "![",
    "<http://a.b>",
    "xn--8c9bk9h",
    "localhost",
    "[::1]",
    "1.2.3.4",
    ":80",
    "?q",
    "#f",
];

fn text(r: &mut Rng) -> Vec<u16> {
    let mut t = Vec::new();
    for _ in 0..r.below(14) {
        if r.chance(40) {
            t.extend(js::to_utf16(r.pick(PIECES)));
        } else {
            t.push(*r.pick(UNITS));
        }
    }
    t
}

/// A Rust string: what `parse` and the tokenizer take (lone surrogates become U+FFFD).
fn document(r: &mut Rng) -> String {
    let mut s = String::new();
    for _ in 0..r.below(6) {
        s += &js::to_string(&text(r));
        if r.chance(30) {
            s += r.pick(&["\n", "\n\n", "\n> ", "\n- ", "\n    ", " "]);
        }
    }
    s
}

fn position(r: &mut Rng, len: usize) -> usize {
    match r.below(8) {
        0 => 0,
        1 => len,
        2 => len + 1,
        3 => len + 7,
        4 => usize::MAX,
        5 => usize::MAX / 2,
        _ => r.below(len + 2),
    }
}

fn int(r: &mut Rng) -> i64 {
    match r.below(8) {
        0 => i64::MIN,
        1 => i64::MAX,
        2 => -1,
        3 => -(r.below(20) as i64),
        4 => i64::from(u32::MAX) + 1,
        _ => r.below(30) as i64,
    }
}

fn code(r: &mut Rng) -> u32 {
    match r.below(6) {
        0 => u32::MAX,
        1 => 0x110000,
        2 => 0x10FFFF,
        3 => 0xD800 + r.below(0x800) as u32,
        _ => r.next() as u32 % 0x11_0000,
    }
}

fn number(r: &mut Rng) -> f64 {
    *r.pick(&[
        0.0,
        -0.0,
        1.0,
        -1.0,
        2.5,
        1e308,
        -1e308,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        3.0,
        100.0,
        4294967296.0,
        1e-7,
    ])
}

fn name(r: &mut Rng) -> String {
    let names = [
        "default",
        "commonmark",
        "zero",
        "",
        "nope",
        "Default",
        "__proto__",
        "constructor",
        "toString",
        "paragraph",
        "text",
        "linkify",
        "emphasis",
        "balance_pairs",
        "text_join",
        "inline",
        "block",
        "annotations",
        "containers",
        "markdoc_delimiters",
        "table",
        "list",
        "fence",
        "smartquotes",
    ];
    if r.chance(80) {
        (*r.pick(&names)).to_owned()
    } else {
        js::to_string(&text(r))
    }
}

fn names(r: &mut Rng) -> Vec<String> {
    (0..r.below(4)).map(|_| name(r)).collect()
}

fn json(r: &mut Rng, depth: usize) -> Value {
    match r.below(if depth == 0 { 5 } else { 7 }) {
        0 => Value::Null,
        1 => Value::Bool(r.chance(50)),
        2 => serde_json::Number::from_f64(number(r)).map_or(Value::Null, Value::Number),
        3 => Value::from(r.below(200) as i64 - 50),
        4 => Value::String(js::to_string(&text(r))),
        5 => Value::Array((0..r.below(5)).map(|_| json(r, depth - 1)).collect()),
        _ => Value::Object(
            (0..r.below(5))
                .map(|_| (name(r), json(r, depth - 1)))
                .collect(),
        ),
    }
}

/// Options in markdown-it's and Markdoc's names, with values of every shape.
fn options(r: &mut Rng) -> Map<String, Value> {
    let keys = [
        "html",
        "xhtmlOut",
        "breaks",
        "langPrefix",
        "linkify",
        "typographer",
        "quotes",
        "maxNesting",
        "allowIndentation",
        "allowComments",
        "allowLinkValidation",
        "linkValidationOptions",
        "highlight",
        "__proto__",
    ];
    let mut o = Map::new();
    for _ in 0..r.below(8) {
        let key = if r.chance(85) {
            (*r.pick(&keys)).to_owned()
        } else {
            name(r)
        };
        let value = match key.as_str() {
            "maxNesting" if r.chance(60) => {
                serde_json::Number::from_f64(number(r)).map_or(Value::Null, Value::Number)
            }
            "quotes" if r.chance(50) => Value::String(js::to_string(&text(r))),
            "linkValidationOptions" if r.chance(50) => {
                let mut v = Map::new();
                v.insert("validatedProtocols".into(), json(r, 2));
                Value::Object(v)
            }
            _ if r.chance(50) => Value::Bool(true),
            _ => json(r, 2),
        };
        o.insert(key, value);
    }
    o
}

fn markdown_it(r: &mut Rng) -> MarkdownIt {
    let preset = *r.pick(&["default", "commonmark", "zero"]);
    let mut md = MarkdownIt::with_options(preset, &options(r)).expect("a preset");
    if r.chance(30) {
        let _ = md.disable(&names(r), r.chance(50));
    }
    if r.chance(30) {
        let _ = md.enable(&names(r), r.chance(50));
    }
    md
}

fn units_u32(r: &mut Rng) -> Vec<u32> {
    (0..r.below(6)).map(|_| code(r)).collect()
}

fn linkify_options(r: &mut Rng) -> LinkifyOptions {
    LinkifyOptions {
        fuzzy_link: r.chance(50),
        fuzzy_email: r.chance(50),
        fuzzy_ip: r.chance(50),
        triple_dash: r.chance(50),
        url_auth: r.chance(50),
        max_length: *r.pick(&[0, 1, 2, 5, 10000, usize::MAX]),
    }
}

fn delimiter(r: &mut Rng) -> Delimiter {
    Delimiter {
        marker: *r.pick(&[0x2A, 0x5F, 0x7E, 0x61]),
        length: *r.pick(&[0, 1, 2, 3, usize::MAX]),
        token: position(r, 4),
        end: int(r),
        open: r.chance(50),
        close: r.chance(50),
    }
}

/// A block rule of a plugin that pushes what a careless plugin might: an `inline` token
/// without children, a closing token with no opening one.
fn careless_block(state: &mut StateBlock, start_line: usize, _end: usize, silent: bool) -> bool {
    if silent || !state.src.starts_with(&[0x21, 0x21]) {
        return false;
    }
    state.push("inline", "", 0);
    state.push("x_close", "", -1);
    state.line = start_line + 1;
    true
}

/// An inline rule of a plugin that pushes closing tokens, and delimiters pointing anywhere.
fn careless_inline(state: &mut StateInline, silent: bool) -> bool {
    match js::char_code_at(&state.src, state.pos) {
        Some(0x24) if !silent => {
            state.push("x_close", "", -1);
            state.pos += 1;
            true
        }
        Some(0x5E) if !silent => {
            let token = state.tokens.len();
            state.push("text", "", 0).content = "*".into();
            let end = (state.pos as i64 % 5) - 2;
            if state.delimiters.is_some() {
                state.push_delimiter(Delimiter {
                    marker: 0x2A,
                    length: 1,
                    token: token + (state.pos % 3),
                    end,
                    open: true,
                    close: false,
                });
            }
            state.pos += 1;
            true
        }
        _ => false,
    }
}

/// A named batch of calls on one random source.
type Call = (&'static str, fn(&mut Rng));

const CALLS: &[Call] = &[
    ("js", |r| {
        let (t, u) = (text(r), text(r));
        let (p, q) = (position(r, t.len()), position(r, t.len()));
        let (i, j) = (int(r), int(r));
        js::char_code_at(&t, p);
        js::code_point_at(&t, p);
        js::from_char_code(&units_u32(r));
        let _ = js::from_code_point(&units_u32(r));
        js::slice(&t, i, r.chance(50).then_some(j));
        js::slice_to(&t, p, q);
        js::substring(&t, i, r.chance(50).then_some(j));
        js::substr(&t, i, r.chance(50).then_some(j));
        js::index_of(&t, &u, p);
        js::last_index_of(&t, &u, r.chance(50).then_some(q));
        js::starts_with(&t, &u, p);
        js::ends_with(&t, &u, r.chance(50).then_some(q));
        js::includes(&t, &u, p);
        js::split(&t, &u);
        js::trim(&t);
        js::to_lower_case(&t);
        js::to_upper_case(&t);
        js::number_to_string(f64::from_bits(r.next()));
        js::number_to_string(number(r));
        js::parse_int(&t, r.below(40) as u32);
        js::string_to_number(&t);
        js::is_js_space(code(r));
    }),
    ("common", |r| {
        let t = text(r);
        let c = code(r);
        let v: Vec<u8> = (0..r.below(5)).map(|_| r.below(256) as u8).collect();
        utils::array_replace_at(&v, position(r, v.len()), vec![1, 2]);
        utils::is_valid_entity_code(c);
        utils::from_code_point(c);
        utils::unescape_md(&t);
        utils::unescape_all(&t);
        utils::escape_html(&js::to_string(&t));
        utils::escape_re(&js::to_string(&t));
        utils::is_space(c);
        utils::is_white_space(c);
        utils::is_punct_char(&t);
        utils::is_punct_char_code(c);
        utils::is_md_ascii_punct(c);
        utils::normalize_reference(&t);
        utils::ascii_trim(&t);
        utils::decode_html_strict(&t);
        entities::lookup(&js::to_string(&t));
        ucmicro::is_p(c);
        ucmicro::is_s(c);
        html_re::html_tag_re(&t);
        html_re::html_open_close_tag_re(&t);
        html_re::html_open_close_tag_re_to_end(&t);
    }),
    ("helpers", |r| {
        let t = text(r);
        let (p, q) = (position(r, t.len()), position(r, t.len()));
        let first = parse_link_title(&t, p, q, None);
        parse_link_title(&t, q, p, Some(&first));
        parse_link_destination(&t, p, q);
        let md = markdown_it(r);
        let mut env = Env::default();
        let mut state = StateInline::new(t.clone(), &md, &mut env, Vec::new());
        if r.chance(50) {
            state.pos_max = state.pos_max.min(q);
        }
        parse_link_label(&mut state, p.min(t.len()), r.chance(50));
    }),
    ("mdurl", |r| {
        let t = text(r);
        let exclude = js::to_string(&text(r));
        let url = mdurl::parse(&t, r.chance(50));
        mdurl::format(&url);
        let mut any = mdurl::Url::default();
        for part in [
            &mut any.protocol,
            &mut any.auth,
            &mut any.port,
            &mut any.hostname,
            &mut any.hash,
            &mut any.search,
            &mut any.pathname,
        ] {
            if r.chance(50) {
                *part = Some(text(r));
            }
        }
        any.slashes = r.chance(50).then_some(r.chance(50));
        mdurl::format(&any);
        mdurl::encode(
            &t,
            r.chance(50).then_some(exclude.as_str()),
            r.chance(50).then_some(r.chance(50)),
        );
        mdurl::decode(&t, r.chance(50).then_some(exclude.as_str()));
    }),
    ("punycode", |r| {
        let t = text(r);
        punycode::ucs2_decode(&t);
        let _ = punycode::ucs2_encode(&units_u32(r));
        let _ = punycode::decode(&t);
        let _ = punycode::encode(&t);
        let _ = punycode::to_unicode(&t);
        let _ = punycode::to_ascii(&t);
    }),
    ("linkify", |r| {
        let t = text(r);
        let p = position(r, t.len());
        let l = LinkifyIt::with_options(linkify_options(r));
        let _ = l.test(&t);
        let _ = l.test_schema_at(&t, &text(r), p);
        let _ = l.test_schema_at(&t, &js::to_utf16(r.pick(&["http:", "//", "mailto:"])), p);
        let _ = l.match_all(&t);
        let _ = l.match_at_start(&t);
        let mut m = Match {
            schema: text(r),
            index: p,
            last_index: position(r, t.len()),
            raw: text(r),
            text: text(r),
            url: text(r),
        };
        l.normalize(&mut m);
        let re = REBuilder::new(REBuilderOptions {
            triple_dash: r.chance(50),
            fuzzy_ip: r.chance(50),
            schema_names: (0..r.below(3)).map(|_| text(r)).collect(),
            tlds: (0..r.below(3)).map(|_| text(r)).collect(),
            url_auth: r.chance(50),
            max_length: *r.pick(&[0, 1, 3, usize::MAX]),
        });
        re.schema_search(&t, p);
        re.schema_at_start(&t);
        re.http_validator(&t, p);
        re.relative_proto_validator(&t, p);
        re.mailto_validator(&t, p);
        re.host_terminator(&t, p);
        re.path(&t, p);
        re.fuzzy_mail_host_search(&t, p);
        re.fuzzy_link_search(&t, p);
        rebuilder::mail_name_validator(&t);
        rebuilder::ipv6_address(&js::to_string(&t));
        rebuilder::zpcc_len(&t, p);
        rebuilder::pseudo_letter_len(&t, p);
        let c = *r.pick(UNITS);
        rebuilder::canon(c);
        rebuilder::eq_ci(c, *r.pick(UNITS));
    }),
    ("markdown_it", |r| {
        let _ = MarkdownIt::new(&name(r));
        let _ = MarkdownIt::with_options(&name(r), &options(r));
        let mut md = markdown_it(r);
        md.set(&options(r));
        let _ = md.configure_named(&name(r));
        let _ = md.enable(&names(r), r.chance(50));
        let _ = md.disable(&names(r), r.chance(50));
        let _ = md.inline.ruler.enable_only(&names(r), r.chance(50));
        let src = document(r);
        let mut env = Env::default();
        md.parse(&src, &mut env);
        let _ = md.try_parse(&src, &mut Env::default());
        let _ = md.try_parse_inline(&src, &mut Env::default());
        md.parse_inline(&src, &mut env);
        let t = text(r);
        md.validate_link(&t);
        md.normalize_link(&t);
        md.normalize_link_text(&t);
    }),
    ("rulers", |r| {
        let mut md = markdown_it(r);
        let rule: BlockRule = careless_block;
        let inline: InlineRule = careless_inline;
        let alt: Vec<String> = names(r);
        let alt: Vec<&str> = alt.iter().map(String::as_str).collect();
        let _ = md.block.ruler.at(&name(r), rule, &alt);
        let _ = md.block.ruler.before(&name(r), &name(r), rule, &alt);
        let _ = md.block.ruler.after(&name(r), &name(r), rule, &alt);
        md.block.ruler.push(&name(r), rule, &alt);
        let _ = md.inline.ruler.before(&name(r), &name(r), inline, &alt);
        md.inline.ruler.push(&name(r), inline, &alt);
        let _ = md.block.ruler.enable(&names(r), r.chance(50));
        let _ = md.block.ruler.disable(&names(r), r.chance(50));
        md.block.ruler.get_rules(&name(r));
        md.inline.ruler.get_rules(&name(r));
        // Markdoc's plugins, on any markdown-it
        if r.chance(50) {
            let _ = plugins::annotations::plugin(&mut md);
            let _ = plugins::frontmatter::plugin(&mut md);
        }
        if r.chance(30) {
            let _ = plugins::comments::plugin(&mut md);
        }
        if r.chance(30) {
            plugins::link::plugin(&mut md, &LinkPluginOptions::from_json(&json(r, 2)));
        }
        let src = format!("!!{}\n\n$^*^a* $ ^{}", document(r), document(r));
        let _ = md.try_parse(&src, &mut Env::default());
        let _ = md.try_parse_inline(&src, &mut Env::default());
    }),
    ("token_and_options", |r| {
        let mut t = Token::new(&name(r), &name(r), *r.pick(&[-1, 0, 1, i8::MIN, i8::MAX]));
        t.attr_push((name(r), AttrValue::Num(number(r))));
        t.attr_set(&name(r), name(r).as_str());
        t.attr_join(&name(r), number(r));
        t.attr_get(&name(r));
        t.attr_index(&name(r));
        let mut o: Options = super::presets::default::options();
        for (k, v) in options(r) {
            o.assign(&k, &v);
        }
        let _ = o.quotes.get(position(r, 4));
        let _ = Quotes::Null.get(position(r, 4));
        let mut env = Env::default();
        env.throw(super::JsError::type_error(name(r)));
    }),
    ("states", |r| {
        let md = markdown_it(r);
        let mut env = Env::default();
        let src = text(r);
        let len = src.len();
        let mut state = StateBlock::new(src, &md, &mut env, Vec::new());
        let lines = state.line_max;
        let (p, q) = (position(r, len), position(r, len));
        let (a, b) = (position(r, lines), position(r, lines));
        state.is_empty(a);
        state.skip_empty_lines(a);
        state.skip_spaces(p);
        state.skip_spaces_back(p, q);
        state.skip_chars(p, *r.pick(UNITS));
        state.skip_chars_back(p, *r.pick(UNITS), q);
        state.get_lines(a, b.min(a.saturating_add(r.below(9))), int(r), r.chance(50));
        state.get_lines(a, b, int(r), r.chance(50));
        // the block rules and the block parser, on any lines
        let rules: Vec<BlockRule> = md.block.ruler.get_rules("").to_vec();
        for rule in rules {
            let mut env = Env::default();
            let mut state = StateBlock::new(text(r), &md, &mut env, Vec::new());
            let lines = state.line_max;
            let (a, b) = (position(r, lines), position(r, lines));
            rule(&mut state, a, b, r.chance(50));
        }
        let mut env = Env::default();
        let mut state = StateBlock::new(text(r), &md, &mut env, Vec::new());
        let lines = state.line_max;
        let (a, b) = (position(r, lines), position(r, lines));
        md.block.tokenize(&mut state, a, b);
        let mut env = Env::default();
        let src = text(r);
        let mut state = StateInline::new(src.clone(), &md, &mut env, Vec::new());
        state.scan_delims(position(r, src.len()), r.chance(50));
        state.push(&name(r), "", *r.pick(&[-1, 0, 1]));
        state.push_delimiter(delimiter(r));
        state.push(&name(r), "", -1);
        state.push_delimiter(delimiter(r));
        md.inline.tokenize(&mut state);
    }),
    ("tokenizer", |r| {
        let mut config = options(r);
        if r.chance(30) {
            config.insert("linkValidationOptions".into(), json(r, 3));
        }
        let options = TokenizerOptions::from_json(&config).expect("never fails");
        let tokenizer = Tokenizer::new(options);
        let _ = tokenizer.tokenize(&document(r));
        LinkPluginOptions::from_json(&json(r, 3));
        let t = text(r);
        tk::is_identifier(&js::to_string(&t));
        tk::find_tag_end(&t, position(r, t.len()));
        let protocols: Vec<String> = names(r);
        tk::contains_markdoc_tag_in_url(&t, &protocols);
        tk::text_before_first_closed_tag(&t);
        let _ = tk::parse_tags(&t, position(r, 3));
    }),
];

#[test]
fn every_public_function_takes_any_argument() {
    let rounds: usize = std::env::var("MDIT_ROBUSTNESS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(300);
    let seed: u64 = std::env::var("MDIT_ROBUSTNESS_SEED")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(1);
    let trace = std::env::var_os("MDIT_ROBUSTNESS_TRACE").is_some();
    // panics on this thread are expected: keep their place, print nothing (other threads'
    // panics go to the hook there was)
    thread_local! {
        static FUZZING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        static PLACE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if FUZZING.with(std::cell::Cell::get) {
            let place = info.location().map(|l| l.to_string()).unwrap_or_default();
            PLACE.with(|p| *p.borrow_mut() = place);
        } else {
            previous(info);
        }
    }));
    FUZZING.with(|f| f.set(true));
    // (call, message) -> (count, first case)
    let mut failures: std::collections::BTreeMap<(&str, String), (usize, u64)> =
        std::collections::BTreeMap::new();
    for round in 0..rounds {
        for (k, (name, call)) in CALLS.iter().enumerate() {
            let case = seed
                .wrapping_mul(1_000_003)
                .wrapping_add((round * CALLS.len() + k) as u64)
                | 1;
            let mut r = Rng(case);
            if trace {
                eprintln!("{name} (Rng({case}))");
            }
            if let Err(e) = catch_unwind(AssertUnwindSafe(|| call(&mut r))) {
                let message = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                    .unwrap_or_default();
                let place = PLACE.with(|p| p.borrow().clone());
                failures
                    .entry((name, format!("{place}: {message}")))
                    .or_insert((0, case))
                    .0 += 1;
            }
        }
    }
    FUZZING.with(|f| f.set(false));
    drop(std::panic::take_hook());
    let report: Vec<String> = failures
        .iter()
        .map(|((name, message), (count, case))| {
            format!("{count} x {name}: {message} (first: Rng({case}))")
        })
        .collect();
    assert!(
        failures.is_empty(),
        "panics in {} rounds of {} calls:\n{}",
        rounds,
        CALLS.len(),
        report.join("\n")
    );
}
