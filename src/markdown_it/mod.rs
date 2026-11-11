//! A port of markdown-it 15.0.1, patched with Markdoc's `allowIndentation` option.
//!
//! This is a line-by-line translation of markdown-it's TypeScript (one Rust file per TS
//! file, same names): same rule order, same branches, same quirks. The goal is a token
//! stream identical, field by field, to the one the JavaScript produces.
//!
//! Strings follow JS semantics: text that rules index is `Vec<u16>` / `&[u16]` and every
//! position is a UTF-16 code-unit index (see [`js`]). Token fields and `Env` hold Rust
//! `String`s, converted with [`js::to_string`].
//!
//! This file is the port of markdownit.ts (the `MarkdownIt` class). `md.linkify` is a port of
//! linkify-it 6.1.0 ([`linkify_it`]), the version markdown-it 15.0.1 depends on.
//!
//! Where JS throws, the port records the exception on the environment and stops (see
//! [`JsError`]); no document, option or other argument of a public function makes it panic
//! (the test `robustness::every_public_function_takes_any_argument` calls each one with
//! arbitrary arguments).
//!
//! # Divergences from upstream
//!
//! - **Recursion depth.** markdown-it recurses once per level of nested blockquotes, list
//!   items, link texts, image descriptions and link-label scans, bounded by `maxNesting`
//!   (100 by default) and by V8's stack. With `maxNesting` raised (1e9), the port charges
//!   each level its share of node's stack ([`cost`], on [`crate::depth`]'s budget) and throws
//!   V8's `RangeError: Maximum call stack size exceeded` where a fresh node 22 process does,
//!   its stack growing as it needs to; node goes deeper once warmed up
//!   (`findings/conformance/work/depth/node_shapes.cjs`):
//!
//!   | nested | the port | node: a fresh process | after 300 / 3,000 documents |
//!   | --- | --- | --- | --- |
//!   | blockquotes | 1,789 | 1,717 (1,789 after 2,000 paragraphs) | 3,296 / 3,212 |
//!   | list items | 1,690 | 1,602 (1,690 nested by indentation) | 2,363 to 3,479 |
//!   | images | 1,642 | 1,561 (1,642 after 2,000 paragraphs) | 1,834 to 2,457 |
//!   | unclosed `[` | 1,604 | 1,604 | 3,240 |
//!   | `> - ` pairs | 869 | 828 | 1,423 / 1,692 |
//!   | `[![` pairs | 811 | 791 to 793 | 1,111 to 1,278 |
//!
//!   One more `>` per line, 3,239 lines deep, takes a fresh node 3,239 blockquotes (V8 has
//!   compiled the rule while it scanned the lines); the port stops at 1,789. Where the stack
//!   cannot grow ([`crate::depth::can_grow`]: the `stack-growth` feature off, or wasm), the
//!   port stops at [`MAX_DEPTH`] (160) nested calls instead: 159 levels of every construct
//!   (79 pairs), the host's limit there. Tests: `next::tokenizer::stack`.
//! - **Lone surrogates.** A token's strings are Rust `String`s, which cannot hold half of a
//!   surrogate pair (the user's decision: tokens, the AST and messages stay `String`s); JS
//!   strings can, and markdown-it makes them from well-formed text in three ways: a `quotes`
//!   option of astral characters (smartquotes inserts `quotes[i]`, one code unit: half of a
//!   pair); an `xn--` label that punycode decodes to a surrogate code point (the text of an
//!   autolink or a linkify link, from `normalizeLinkText`); and linkify-it's `maxLength`
//!   (10,000 code units past a schema) cutting a pair, which ends a link's text with the high
//!   half and starts the text after the link with the low half. Where node's string has a
//!   lone surrogate, the port's `content` has U+FFFD: the bytes node writes when it encodes
//!   the string as UTF-8. Inside the engine nothing is lossy: a token whose content holds a
//!   lone surrogate keeps its exact UTF-16 beside `content` (a private field), and every rule
//!   that reads a content back (linkify, replacements, smartquotes reading the code unit
//!   beside a quote in its own or a neighbouring token, the joins of adjacent text tokens)
//!   reads that, so it decides as node does and halves that meet pair again. What stays is
//!   the boundary itself: halves in two tokens that a later stage puts side by side (Markdoc's
//!   HTML renderer joins text nodes, so with astral quotes `"{% .a %}"` or `"{% $x %}"` with
//!   `x` empty is the emoji in node's HTML) are two U+FFFD in the port's.
//!
//!   Tests: `tests::lone_surrogates_the_engine_makes_are_u_fffd_and_halves_that_meet_pair_again`,
//!   `tests::a_lone_surrogate_the_engine_made_reads_back_as_node_reads_it`.
//! - **Rules and parsers past their state.** The block rules and `ParserBlock::tokenize`
//!   take line numbers, which markdown-it's own callers keep within the state; past
//!   `state.line_max` JS reads `undefined` marks and goes on with `NaN`s. There the port's
//!   rules match nothing and the end line is `line_max`, where JS's `NaN`s make other
//!   things (the paragraph rule an empty paragraph, a loop up to the end line). A state is
//!   taken as the engine makes it: fields a caller sets out of step with each other (a mark
//!   past the source, `pos_max` past it) are not checked.

// The port mirrors the TypeScript shape on purpose; these lints would pull it away from it.
#![allow(
    clippy::collapsible_else_if,
    clippy::collapsible_if,
    clippy::comparison_chain,
    clippy::if_same_then_else,
    clippy::int_plus_one,
    clippy::manual_range_contains,
    clippy::needless_late_init,
    clippy::needless_range_loop,
    clippy::nonminimal_bool,
    clippy::too_many_arguments,
    clippy::while_let_loop
)]

pub mod common;
pub mod helpers;
pub mod js;
pub mod linkify_it;
pub mod mdurl;
pub mod parser_block;
pub mod parser_core;
pub mod parser_inline;
pub mod presets;
pub mod punycode;
pub mod ruler;
pub mod rules_block;
pub mod rules_core;
pub mod rules_inline;
pub mod token;

#[cfg(test)]
mod robustness;

use std::collections::HashMap;

use serde_json::Value;

pub use parser_block::{BlockRule, ParserBlock};
pub use parser_core::{CoreRule, ParserCore};
pub use parser_inline::{InlineRule, InlineRule2, ParserInline};
pub use presets::{ComponentRules, Components, Preset};
pub use ruler::{Rule, Ruler, RulerError};
pub use rules_block::state_block::StateBlock;
pub use rules_core::state_core::StateCore;
pub use rules_inline::state_inline::{
    Delimiter, DelimiterList, ScannedDelims, StateInline, TokenMeta,
};
pub use token::{AttrValue, PseudoShape, PseudoToken, Token};

/// An `Error` markdownit.ts (or ruler.ts, through it) throws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownItError(pub String);

impl std::fmt::Display for MarkdownItError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MarkdownItError {}

impl From<RulerError> for MarkdownItError {
    fn from(e: RulerError) -> Self {
        MarkdownItError(e.0)
    }
}

/// A JavaScript exception, by its `name` (`TypeError`, `RangeError`, `Error`, ...) and its
/// `message`: what markdown-it, or a plugin running in it, throws.
///
/// A rule cannot unwind the parse the way a JS `throw` does, so it records the exception on
/// the environment ([`Env::throw`]) and returns; from then on every parser loop stops at
/// its next step, and the tokens the parse returns are not upstream's (JS returns none).
/// [`MarkdownIt::try_parse`] gives the exception instead of the tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsError {
    pub name: String,
    pub message: String,
}

impl JsError {
    pub fn new(name: &str, message: impl Into<String>) -> Self {
        JsError {
            name: name.to_owned(),
            message: message.into(),
        }
    }

    /// `new Error(message)`
    pub fn error(message: impl Into<String>) -> Self {
        Self::new("Error", message)
    }

    /// `new TypeError(message)`
    pub fn type_error(message: impl Into<String>) -> Self {
        Self::new("TypeError", message)
    }

    /// `new RangeError(message)`
    pub fn range_error(message: impl Into<String>) -> Self {
        Self::new("RangeError", message)
    }

    /// What V8 throws when its stack runs out, and what the port throws past
    /// [`MAX_DEPTH`].
    pub fn stack_overflow() -> Self {
        Self::range_error("Maximum call stack size exceeded")
    }
}

impl std::fmt::Display for JsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.message)
    }
}

impl std::error::Error for JsError {}

/// `options.quotes`, read as smartquotes.ts reads it: `quotes[i]`.
#[derive(Debug, Clone, PartialEq)]
pub enum Quotes {
    /// A string: `quotes[i]` is its `i`-th UTF-16 code unit.
    Str(String),
    /// An array: `quotes[i]` is its `i`-th element, as `+` makes it a string. A number or a
    /// boolean is `Array(vec![])`: every `quotes[i]` is `undefined`.
    Array(Vec<String>),
    /// Any other object: `quotes[i]` is its property `"i"`, as `+` makes it a string.
    Object(HashMap<String, String>),
    /// `null`: reading `quotes[i]` throws a TypeError, so smartquotes throws once it
    /// replaces a pair of quotes.
    Null,
}

impl Quotes {
    /// `quotes[i]`; `Ok(None)` is `undefined`. For `null`, the TypeError JS throws.
    pub fn get(&self, i: usize) -> Result<Option<Vec<u16>>, JsError> {
        Ok(match self {
            Quotes::Str(s) => js::to_utf16(s).get(i).map(|&u| vec![u]),
            Quotes::Array(a) => a.get(i).map(|s| js::to_utf16(s)),
            Quotes::Object(o) => o.get(&i.to_string()).map(|s| js::to_utf16(s)),
            Quotes::Null => {
                return Err(JsError::type_error(format!(
                    "Cannot read properties of null (reading '{i}')"
                )));
            }
        })
    }
}

/// `MarkdownItOptions` (`md.options`), plus Markdoc's `allowIndentation`.
///
/// `highlight` is renderer-only and not ported. Keys `set` does not know are kept in
/// `extra`, as `Object.assign` keeps them on the JS options object (Markdoc passes its own
/// config there, e.g. `allowComments`).
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// Enable HTML tags in source.
    pub html: bool,
    /// Use '/' to close single tags (`<br />`).
    pub xhtml_out: bool,
    /// Convert '\n' in paragraphs into `<br>`.
    pub breaks: bool,
    /// CSS language prefix for fenced blocks.
    pub lang_prefix: String,
    /// Autoconvert URL-like texts to links (with `md.linkify`, linkify-it).
    pub linkify: bool,
    /// Enable some language-neutral replacements + quotes beautification.
    pub typographer: bool,
    /// Double + single quotes replacement pairs, when typographer enabled.
    pub quotes: Quotes,
    /// Internal protection, recursion limit. A JS number, compared with `state.level` as JS
    /// compares (`level >= maxNesting` in the block parser, `level < maxNesting` in the
    /// inline one), so that `set` can give it any value: `NaN` lets block nesting go on
    /// and stops every inline rule, as in JS.
    pub max_nesting: f64,
    /// Markdoc's patch: when true, the four-column indentation checks of table, fence,
    /// blockquote, hr, list, reference, html_block, heading and paragraph are skipped.
    /// markdown-it's presets do not set it (`undefined`, i.e. false).
    pub allow_indentation: bool,
    /// Any other key given to `set`.
    pub extra: serde_json::Map<String, Value>,
}

/// JS truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn js_string_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => js::number_to_string(n.as_f64().unwrap_or(f64::NAN)),
        Value::Array(a) => a
            .iter()
            .map(|x| {
                if x.is_null() {
                    String::new()
                } else {
                    js_string_of(x)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// `Number(v)` of a JSON value (ECMAScript ToNumber), as a comparison with a number applies it.
fn js_to_number(v: &Value) -> f64 {
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(_) | Value::Array(_) | Value::Object(_) => {
            js::string_to_number(&js::to_utf16(&js_string_of(v)))
        }
    }
}

impl Options {
    /// One key of `Object.assign(this.options, options)`, with the JS (camelCase) name.
    /// Booleans take the value's truthiness, as the rules only test them; `maxNesting` takes
    /// `Number(value)`, which is what the comparisons with `state.level` use.
    pub fn assign(&mut self, key: &str, value: &Value) {
        match key {
            "html" => self.html = truthy(value),
            "xhtmlOut" => self.xhtml_out = truthy(value),
            "breaks" => self.breaks = truthy(value),
            "langPrefix" => self.lang_prefix = js_string_of(value),
            "linkify" => self.linkify = truthy(value),
            "typographer" => self.typographer = truthy(value),
            "quotes" => {
                self.quotes = match value {
                    Value::String(s) => Quotes::Str(s.clone()),
                    Value::Array(a) => Quotes::Array(a.iter().map(js_string_of).collect()),
                    Value::Object(o) => Quotes::Object(
                        o.iter()
                            .map(|(k, v)| (k.clone(), js_string_of(v)))
                            .collect(),
                    ),
                    Value::Null => Quotes::Null,
                    // `(5)[0]` and `true[0]` are undefined
                    Value::Number(_) | Value::Bool(_) => Quotes::Array(Vec::new()),
                }
            }
            "maxNesting" => self.max_nesting = js_to_number(value),
            "allowIndentation" => self.allow_indentation = truthy(value),
            _ => {
                self.extra.insert(key.to_owned(), value.clone());
            }
        }
    }
}

/// `env.references[label]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub href: String,
    pub title: String,
}

/// The `env` object passed between rules.
///
/// `references` is `None` until the first reference definition, because link.ts and
/// image.ts test `typeof state.env.references === 'undefined'` before looking a label up.
///
/// Two fields are the port's own, not JS's: `thrown`, the exception the parse threw (see
/// [`JsError`]), and the engine's recursion depth (see [`MAX_DEPTH`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Env {
    /// `env.references`, keyed by normalized label; the first definition of a label wins.
    pub references: Option<HashMap<String, Reference>>,
    /// Any other key plugins keep on `env`.
    pub extra: serde_json::Map<String, Value>,
    /// The exception the parse threw, if it threw: the first one, where JS would have
    /// stopped. Once it is set the parsers stop, so the tokens are not upstream's.
    pub thrown: Option<JsError>,
    /// How deep the engine's recursive calls are nested now (block and inline `tokenize`,
    /// `skipToken`); see [`MAX_DEPTH`].
    depth: usize,
}

impl Env {
    /// Records `error` as thrown, unless something was thrown already (the first exception
    /// is the one that escapes).
    pub fn throw(&mut self, error: JsError) {
        if self.thrown.is_none() {
            self.thrown = Some(error);
        }
    }

    /// Whether something was thrown.
    pub fn has_thrown(&self) -> bool {
        self.thrown.is_some()
    }

    /// Enters one more level of the engine's recursion: `false`, with the RangeError V8
    /// throws recorded, past [`MAX_DEPTH`] where the stack cannot grow (the caller then
    /// returns at once, without recursing and without calling [`Env::leave`]); `false` too
    /// once something was thrown. Where the stack grows ([`crate::depth::can_grow`]), the
    /// depth is bounded by the constructs' [`cost`]s instead (see [`Env::level`]).
    pub(crate) fn enter(&mut self) -> bool {
        if self.thrown.is_some() {
            return false;
        }
        if self.depth >= MAX_DEPTH && !crate::depth::can_grow() {
            self.throw(JsError::stack_overflow());
            return false;
        }
        self.depth += 1;
        true
    }

    /// Opens a level of a construct that recurses (`cost` is one of [`cost`]'s) on the
    /// thread's depth budget ([`crate::depth`]): `None`, with the RangeError V8 throws
    /// recorded, where node's stack would run out.
    pub(crate) fn level(&mut self, cost: u64) -> Option<crate::depth::Level> {
        let level = crate::depth::enter(cost);
        if level.is_none() {
            self.throw(JsError::stack_overflow());
        }
        level
    }

    /// Leaves the level [`Env::enter`] entered.
    pub(crate) fn leave(&mut self) {
        self.depth -= 1;
    }
}

/// How deep the engine's own recursion may go where the stack cannot grow (the `stack-growth`
/// feature off, or wasm: see [`crate::depth`]): nested calls of the block parser's `tokenize`
/// (a blockquote or a list item inside another), of the inline parser's `tokenize` and
/// `parse` (link text, image descriptions) and of `skipToken` (the scan of a nested link
/// label). markdown-it bounds these only by `maxNesting` (100 by default, which stays below
/// 105 here) and V8's stack; with a large `maxNesting` they recurse once per level of the
/// input. Past this depth the port throws V8's `RangeError: Maximum call stack size
/// exceeded`, where node goes on until its stack runs out (see "Divergences" in the module
/// docs). Where the stack grows, the constructs' [`cost`]s bound the depth instead, at
/// node's.
///
/// Chosen by measuring a debug build on a thread with 2 MiB of stack: a level costs up to
/// 2.2 KiB (a list item; a blockquote 1.8, an image 2.0, a link-label scan 1.1), the tag
/// grammar at its deepest ([`crate::grammar::MAX_NESTING`]) with the tokenizer around it
/// 1,167 KiB, and 159 nested list items around that deepest tag 1,523 KiB: a quarter of the
/// stack is left.
pub const MAX_DEPTH: usize = 160;

/// What a level of each construct that recurses costs on the thread's depth budget
/// ([`crate::depth`]): its share of node's stack, so that each construct alone nests as deep
/// as in a fresh node 22 process (`maxNesting` raised, `tokenize` called from the top level of
/// a script), and mixed ones (`> - ` pairs, a link around an image, a tag's value inside them)
/// about as deep as node nests them. Measured with
/// `findings/conformance/work/depth/node_shapes.cjs`: the deepest nesting a fresh process
/// takes (and a warmed-up one). A construct's limit moves with what the call did before it
/// nested (V8 compiles the rules while it tokenizes a long document): each cost is the
/// deepest fresh measurement of its construct that no warmed-up process falls below.
pub mod cost {
    use crate::depth::per_level;

    /// A blockquote: `> > x` 1,717 (warm 3,296); after 2,000 paragraphs, 1,789 (warm 2,198),
    /// which is this cost's. (One more `>` per line, 3,239 lines: node has compiled the rule
    /// by then.)
    pub const BLOCKQUOTE: u64 = per_level(1_789);
    /// A list item: `- - x` and `1. 1. x` 1,602 (warm 2,505 to 3,479); nested by indentation,
    /// 1,689 (bullet) and 1,690 (ordered, warm 2,363 to 3,386), which is this cost's.
    pub const LIST: u64 = per_level(1_690);
    /// A link's label, scanned (`[[[x`: 1,604, warm 3,240) or tokenized.
    pub const LINK: u64 = per_level(1_604);
    /// An image's description, scanned or parsed: `![![x](u)](u)` 1,561 (warm 2,457), with
    /// text around each, 1,572 (warm 1,957); after 2,000 paragraphs, 1,642 (warm 2,457),
    /// which is this cost's.
    pub const IMAGE: u64 = per_level(1_642);
}

const RECODE_HOSTNAME_FOR: [&str; 3] = ["http:", "https:", "mailto:"];

/// `MarkdownIt`: the parser (rendering is not ported).
#[derive(Debug, Clone)]
pub struct MarkdownIt {
    /// `md.inline`: the inline rules (`ruler`) and post-processing rules (`ruler2`).
    pub inline: ParserInline,
    /// `md.block`: the block rules.
    pub block: ParserBlock,
    /// `md.core`: the core rules.
    pub core: ParserCore,
    /// `md.linkify`: the linkify-it instance the `linkify` rules use (with `linkify: true`).
    pub linkify: linkify_it::LinkifyIt,
    /// `md.options`.
    pub options: Options,
    /// `md.options` is `undefined`: the instance was made with a preset name `config` only
    /// inherits (`new MarkdownIt('toString')`), which configures nothing (see
    /// [`MarkdownIt::new`]); `options` then holds nothing JS has.
    options_undefined: bool,
}

impl Default for MarkdownIt {
    /// `new MarkdownIt()`: the `"default"` preset.
    fn default() -> Self {
        Self::new("default").expect("the default preset exists")
    }
}

impl MarkdownIt {
    /// The parsers with every rule registered and enabled, before any preset is applied
    /// (the class field initializers of markdownit.ts).
    fn bare() -> Self {
        MarkdownIt {
            inline: ParserInline::new(),
            block: ParserBlock::new(),
            core: ParserCore::new(),
            linkify: linkify_it::LinkifyIt::new(),
            options: presets::default::options(),
            options_undefined: false,
        }
    }

    /// `new MarkdownIt(presetName)`: `"default"`, `"commonmark"` or `"zero"`; a name the
    /// preset table (a plain object) only inherits (`toString`, `constructor`, `__proto__`,
    /// any name `Object.prototype` has) finds a truthy value with no `options` and no
    /// `components`: the instance is made, every rule enabled and `md.options` `undefined`,
    /// so that [`MarkdownIt::try_parse`] throws `TypeError: Cannot read properties of
    /// undefined (reading 'maxNesting')` and [`MarkdownIt::try_set`] `TypeError: Cannot
    /// convert undefined or null to object` ([`MarkdownIt::has_options`] tells). Any other
    /// name is the error JS throws (`Wrong 'markdown-it' preset "name", check name`).
    /// [`MarkdownIt::default`] is `new MarkdownIt()`.
    pub fn new(preset_name: &str) -> Result<Self, MarkdownItError> {
        let mut md = Self::bare();
        if presets::by_name(preset_name).is_none() && presets::is_inherited(preset_name) {
            md.options_undefined = true;
            return Ok(md);
        }
        md.configure_named(preset_name)?;
        Ok(md)
    }

    /// `md.options` is an object (not `undefined`, as [`MarkdownIt::new`] leaves it for a
    /// preset name the table only inherits).
    pub fn has_options(&self) -> bool {
        !self.options_undefined
    }

    /// `set(options)`, or the `TypeError` JS throws when `md.options` is `undefined`
    /// (`Object.assign(undefined, options)`).
    pub fn try_set(
        &mut self,
        options: &serde_json::Map<String, Value>,
    ) -> Result<&mut Self, JsError> {
        if self.options_undefined {
            return Err(JsError::type_error(
                "Cannot convert undefined or null to object",
            ));
        }
        Ok(self.set(options))
    }

    /// `new MarkdownIt(presetName, options)`; `new MarkdownIt(options)` is
    /// `with_options("default", options)`. `options` uses the JS key names (`maxNesting`,
    /// `allowIndentation`, ...). With a preset name the table only inherits, `set` throws
    /// (see [`MarkdownIt::try_set`]): that `TypeError`, as `MarkdownItError`'s text
    /// (`TypeError: Cannot convert undefined or null to object`).
    pub fn with_options(
        preset_name: &str,
        options: &serde_json::Map<String, Value>,
    ) -> Result<Self, MarkdownItError> {
        let mut md = Self::new(preset_name)?;
        md.try_set(options)
            .map_err(|e| MarkdownItError(format!("{}: {}", e.name, e.message)))?;
        Ok(md)
    }

    /// `set(options)`: `Object.assign(this.options, options)`. On an instance whose
    /// `md.options` is `undefined` JS throws: that is [`MarkdownIt::try_set`]'s error, and
    /// this does nothing.
    pub fn set(&mut self, options: &serde_json::Map<String, Value>) -> &mut Self {
        if self.options_undefined {
            return self;
        }
        for (key, value) in options {
            self.options.assign(key, value);
        }
        self
    }

    /// `configure(presetName)`. A name the preset table only inherits configures nothing.
    pub fn configure_named(&mut self, preset_name: &str) -> Result<&mut Self, MarkdownItError> {
        if presets::by_name(preset_name).is_none() && presets::is_inherited(preset_name) {
            return Ok(self);
        }
        let Some(p) = presets::by_name(preset_name) else {
            return Err(MarkdownItError(format!(
                "Wrong 'markdown-it' preset \"{preset_name}\", check name"
            )));
        };
        self.configure(&p)
    }

    /// `configure(preset)`: replace the options and `enableOnly` the listed rules of each
    /// component.
    pub fn configure(&mut self, p: &Preset) -> Result<&mut Self, MarkdownItError> {
        if let Some(options) = &p.options {
            self.options = options.clone();
            self.options_undefined = false;
        }

        if let Some(components) = &p.components {
            if let Some(rules) = components.core.as_ref().and_then(|c| c.rules.as_ref()) {
                self.core.ruler.enable_only(rules, false)?;
            }
            if let Some(rules) = components.block.as_ref().and_then(|c| c.rules.as_ref()) {
                self.block.ruler.enable_only(rules, false)?;
            }
            if let Some(rules) = components.inline.as_ref().and_then(|c| c.rules.as_ref()) {
                self.inline.ruler.enable_only(rules, false)?;
            }

            if let Some(rules2) = components.inline.as_ref().and_then(|c| c.rules2.as_ref()) {
                self.inline.ruler2.enable_only(rules2, false)?;
            }
        }
        Ok(self)
    }

    /// `enable(list, ignoreInvalid)`: enable the named rules in every chain. Unknown names are
    /// an error unless `ignore_invalid` (the known ones are enabled either way, as in JS).
    pub fn enable<S: AsRef<str>>(
        &mut self,
        list: &[S],
        ignore_invalid: bool,
    ) -> Result<&mut Self, MarkdownItError> {
        let mut result: Vec<String> = Vec::new();

        result.extend(self.core.ruler.enable(list, true)?);
        result.extend(self.block.ruler.enable(list, true)?);
        result.extend(self.inline.ruler.enable(list, true)?);

        result.extend(self.inline.ruler2.enable(list, true)?);

        let missed: Vec<&str> = list
            .iter()
            .map(AsRef::as_ref)
            .filter(|name| !result.iter().any(|r| r == name))
            .collect();

        if !missed.is_empty() && !ignore_invalid {
            return Err(MarkdownItError(format!(
                "MarkdownIt. Failed to enable unknown rule(s): {}",
                missed.join(",")
            )));
        }

        Ok(self)
    }

    /// `disable(list, ignoreInvalid)`: as [`MarkdownIt::enable`], turning rules off.
    pub fn disable<S: AsRef<str>>(
        &mut self,
        list: &[S],
        ignore_invalid: bool,
    ) -> Result<&mut Self, MarkdownItError> {
        let mut result: Vec<String> = Vec::new();

        result.extend(self.core.ruler.disable(list, true)?);
        result.extend(self.block.ruler.disable(list, true)?);
        result.extend(self.inline.ruler.disable(list, true)?);

        result.extend(self.inline.ruler2.disable(list, true)?);

        let missed: Vec<&str> = list
            .iter()
            .map(AsRef::as_ref)
            .filter(|name| !result.iter().any(|r| r == name))
            .collect();

        if !missed.is_empty() && !ignore_invalid {
            return Err(MarkdownItError(format!(
                "MarkdownIt. Failed to disable unknown rule(s): {}",
                missed.join(",")
            )));
        }
        Ok(self)
    }

    /// `use(plugin, ...params)`: params are whatever the closure captures.
    pub fn use_plugin(&mut self, plugin: impl FnOnce(&mut MarkdownIt)) -> &mut Self {
        plugin(self);
        self
    }

    /// `parse(src, env)`: the block token stream (`inline` tokens carry their children).
    ///
    /// Where JS throws, `env.thrown` holds the exception afterwards and the tokens are what
    /// the parse had made when it stopped (JS returns none); [`MarkdownIt::try_parse`]
    /// returns the exception instead.
    pub fn parse(&self, src: &str, env: &mut Env) -> Vec<Token> {
        if self.options_undefined {
            // The block parser reads `state.md.options.maxNesting` first.
            return Self::no_options(env);
        }
        let mut state = StateCore::new(js::to_utf16(src), self, env);

        self.core.process(&mut state);

        state.tokens
    }

    /// `parse(src, env)`, or the exception it throws (taken out of `env.thrown`).
    pub fn try_parse(&self, src: &str, env: &mut Env) -> Result<Vec<Token>, JsError> {
        let tokens = self.parse(src, env);
        match env.thrown.take() {
            Some(error) => Err(error),
            None => Ok(tokens),
        }
    }

    /// `parseInline(src, env)`: as `parse`, skipping the block rules; one `inline` token.
    /// Throws as [`MarkdownIt::parse`] does.
    pub fn parse_inline(&self, src: &str, env: &mut Env) -> Vec<Token> {
        if self.options_undefined {
            // The inline parser reads `state.md.options.maxNesting` first.
            return Self::no_options(env);
        }
        let mut state = StateCore::new(js::to_utf16(src), self, env);

        state.inline_mode = true;
        self.core.process(&mut state);

        state.tokens
    }

    /// What `parse` gives when `md.options` is `undefined`: the `TypeError` thrown, no tokens.
    fn no_options(env: &mut Env) -> Vec<Token> {
        env.throw(JsError::type_error(
            "Cannot read properties of undefined (reading 'maxNesting')",
        ));
        Vec::new()
    }

    /// `parseInline(src, env)`, or the exception it throws (taken out of `env.thrown`).
    pub fn try_parse_inline(&self, src: &str, env: &mut Env) -> Result<Vec<Token>, JsError> {
        let tokens = self.parse_inline(src, env);
        match env.thrown.take() {
            Some(error) => Err(error),
            None => Ok(tokens),
        }
    }

    /// `validateLink(url)`: refuse `javascript:`, `vbscript:`, `file:` and most `data:` URLs.
    pub fn validate_link(&self, url: &[u16]) -> bool {
        // url should be normalized at this point, and existing entities are decoded
        let s = js::to_lower_case(js::trim(url));

        // BAD_PROTO_RE = /^(vbscript|javascript|file|data):/ and
        // GOOD_DATA_RE = /^data:image\/(gif|png|jpeg|webp);/ are anchored, ASCII-only prefix
        // tests, matched here by hand.
        let starts = |prefix: &str| js::starts_with(&s, &js::to_utf16(prefix), 0);
        let bad_proto = ["vbscript:", "javascript:", "file:", "data:"]
            .iter()
            .any(|p| starts(p));
        if bad_proto {
            [
                "data:image/gif;",
                "data:image/png;",
                "data:image/jpeg;",
                "data:image/webp;",
            ]
            .iter()
            .any(|p| starts(p))
        } else {
            true
        }
    }

    /// `normalizeLink(url)`: encode a link URL (percent-encoding, punycode hostnames).
    pub fn normalize_link(&self, url: &[u16]) -> Vec<u16> {
        // Each url part should be encoded separately, with proper encoder
        let mut parsed = mdurl::parse(url, true);

        if parsed.hostname.as_ref().is_some_and(|h| !h.is_empty()) {
            // Encode hostnames in urls like:
            // `http://host/`, `https://host/`, `mailto:user@host`, `//host/`
            //
            // We don't encode unknown schemas, because it's likely that we encode
            // something we shouldn't (e.g. `skype:name` treated as `skype:host`)
            //
            if recode_hostname(&parsed)
                && let Ok(hostname) =
                    punycode::to_ascii(parsed.hostname.as_deref().unwrap_or_default())
            {
                parsed.hostname = Some(hostname);
            }
        }

        for part in [
            &mut parsed.auth,
            &mut parsed.hostname,
            &mut parsed.pathname,
            &mut parsed.search,
            &mut parsed.hash,
        ] {
            if let Some(value) = part.as_ref().filter(|v| !v.is_empty()) {
                *part = Some(mdurl::encode(value, None, None));
            }
        }

        mdurl::format(&parsed)
    }

    /// `normalizeLinkText(url)`: decode a link URL for display.
    pub fn normalize_link_text(&self, url: &[u16]) -> Vec<u16> {
        let mut parsed = mdurl::parse(url, true);

        if parsed.hostname.as_ref().is_some_and(|h| !h.is_empty()) {
            // Encode hostnames in urls like:
            // `http://host/`, `https://host/`, `mailto:user@host`, `//host/`
            //
            // We don't encode unknown schemas, because it's likely that we encode
            // something we shouldn't (e.g. `skype:name` treated as `skype:host`)
            //
            if recode_hostname(&parsed)
                && let Ok(hostname) =
                    punycode::to_unicode(parsed.hostname.as_deref().unwrap_or_default())
            {
                parsed.hostname = Some(hostname);
            }
        }

        // add '%' to exclude list because of https://github.com/markdown-it/markdown-it/issues/720
        let exclude = format!("{}%", mdurl::decode::DEFAULT_CHARS);
        mdurl::decode(&mdurl::format(&parsed), Some(&exclude))
    }
}

/// `!parsed.protocol || RECODE_HOSTNAME_FOR.indexOf(parsed.protocol) >= 0` (case-sensitive).
fn recode_hostname(parsed: &mdurl::Url) -> bool {
    match parsed.protocol.as_deref() {
        None | Some([]) => true,
        Some(protocol) => RECODE_HOSTNAME_FOR
            .iter()
            .any(|p| js::to_utf16(p) == protocol),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> Vec<u16> {
        js::to_utf16(x)
    }

    #[test]
    fn presets_and_errors() {
        assert!(MarkdownIt::new("default").is_ok());
        // node: `new MarkdownIt('nope')` throws this Error; so does any other name, the
        // empty one and a preset's name in another case
        for name in ["nope", "", "Default", "Constructor", "__proto"] {
            assert_eq!(
                MarkdownIt::new(name).err(),
                Some(MarkdownItError(format!(
                    "Wrong 'markdown-it' preset \"{name}\", check name"
                )))
            );
        }
        assert_eq!(
            MarkdownIt::new("nope").err(),
            Some(MarkdownItError(
                "Wrong 'markdown-it' preset \"nope\", check name".into()
            ))
        );
        // node (the dist's markdown-it 15.0.1): a name `Object.prototype` has makes an
        // instance whose `options` is undefined: parse and set throw, configure does nothing.
        for name in crate::markdown_it::mdurl::OBJECT_PROTOTYPE_KEYS {
            let mut md = MarkdownIt::new(name).expect("an instance");
            assert!(!md.has_options());
            let mut env = Env::default();
            assert_eq!(
                md.try_parse("# a", &mut env).err(),
                Some(JsError::type_error(
                    "Cannot read properties of undefined (reading 'maxNesting')"
                ))
            );
            assert_eq!(
                md.try_parse_inline("a", &mut env).err(),
                Some(JsError::type_error(
                    "Cannot read properties of undefined (reading 'maxNesting')"
                ))
            );
            assert_eq!(
                md.try_set(&serde_json::Map::new()).err(),
                Some(JsError::type_error(
                    "Cannot convert undefined or null to object"
                ))
            );
            let options = serde_json::json!({"html": true});
            assert_eq!(
                MarkdownIt::with_options(name, options.as_object().unwrap()).err(),
                Some(MarkdownItError(
                    "TypeError: Cannot convert undefined or null to object".into()
                ))
            );
            let mut md = MarkdownIt::default();
            md.configure_named(name).expect("configures nothing");
            assert!(md.has_options() && md.options.max_nesting == 100.0);
        }
        let cm = MarkdownIt::new("commonmark").unwrap();
        assert!(cm.options.html && cm.options.xhtml_out && cm.options.max_nesting == 20.0);
        let zero = MarkdownIt::new("zero").unwrap();
        let names = |r: &[Rule<BlockRule>]| {
            r.iter()
                .filter(|x| x.enabled)
                .map(|x| x.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(zero.block.ruler.rules()), ["paragraph"]);
    }

    #[test]
    fn enable_disable_report_unknown_rules_like_js() {
        let mut md = MarkdownIt::default();
        assert!(md.disable(&["lheading", "code"], false).is_ok());
        assert_eq!(
            md.disable(&["code", "nope", "nada"], false).err(),
            Some(MarkdownItError(
                "MarkdownIt. Failed to disable unknown rule(s): nope,nada".into()
            ))
        );
        assert!(md.enable(&["nope"], true).is_ok());
        assert_eq!(
            md.enable(&["nope"], false).err(),
            Some(MarkdownItError(
                "MarkdownIt. Failed to enable unknown rule(s): nope".into()
            ))
        );
        // `emphasis` and `strikethrough` live in both inline rulers
        md.disable(&["emphasis"], false).unwrap();
        assert!(
            md.inline
                .ruler2
                .rules()
                .iter()
                .any(|r| r.name == "emphasis" && !r.enabled)
        );
    }

    #[test]
    fn set_follows_object_assign() {
        let mut md = MarkdownIt::default();
        let opts: serde_json::Map<String, Value> = serde_json::from_str(
            r#"{"allowIndentation": true, "maxNesting": 7, "quotes": ["a","b","c","d"], "allowComments": true}"#,
        )
        .unwrap();
        md.set(&opts);
        assert!(md.options.allow_indentation);
        assert_eq!(md.options.max_nesting, 7.0);
        assert_eq!(md.options.quotes.get(1), Ok(Some(s("b"))));
        assert_eq!(
            md.options.extra.get("allowComments"),
            Some(&Value::Bool(true))
        );
        // configure() replaces the options object: allowIndentation is gone again
        md.configure_named("default").unwrap();
        assert!(!md.options.allow_indentation && md.options.extra.is_empty());
        assert_eq!(
            Quotes::Str("\u{201c}\u{201d}".into()).get(1),
            Ok(Some(vec![0x201D]))
        );
        assert_eq!(Quotes::Str("ab".into()).get(5), Ok(None));
    }

    #[test]
    fn max_nesting_is_compared_as_a_js_number() {
        // node (new MarkdownIt({maxNesting: V})): the token types of parse('> > > a\n') and
        // of parseInline('*a* [b](c)')'s children (bq = blockquote)
        let types = |v: Value| {
            let mut options = serde_json::Map::new();
            options.insert("maxNesting".into(), v);
            let md = MarkdownIt::with_options("default", &options).unwrap();
            let kinds = |ts: &[Token]| {
                let kinds: Vec<_> = ts
                    .iter()
                    .map(|t| t.kind.replace("blockquote", "bq"))
                    .collect();
                kinds.join(" ")
            };
            let block = md.parse("> > > a\n", &mut Env::default());
            let inline = md.parse_inline("*a* [b](c)", &mut Env::default());
            (kinds(&block), kinds(inline[0].children.as_ref().unwrap()))
        };
        let rich = "em_open text em_close text link_open text link_close";
        let pair = |block: &str, inline: &str| (block.to_owned(), inline.to_owned());
        let two = "bq_open bq_open bq_close bq_close";
        assert_eq!(types(Value::from("2")), pair(two, rich));
        assert_eq!(types(Value::Null), pair("", "text"));
        assert_eq!(types(Value::Bool(true)), pair("bq_open bq_close", rich));
        assert_eq!(types(Value::from(" 0x1 ")), pair("bq_open bq_close", rich));
        let three = "bq_open bq_open bq_open bq_close bq_close bq_close";
        assert_eq!(types(serde_json::json!([3])), pair(three, rich));
        // NaN: the block parser never stops nesting, the inline one never runs a rule
        let deep = "bq_open bq_open bq_open paragraph_open inline paragraph_close bq_close bq_close bq_close";
        assert_eq!(types(Value::from("abc")), pair(deep, "text"));
        assert_eq!(types(serde_json::json!({})), pair(deep, "text"));
    }

    #[test]
    fn quotes_option_is_indexed_as_js_indexes_it() {
        // node: new MarkdownIt({typographer: true, quotes: Q}).parseInline(src, {}), for the
        // text of `"a" 'b'`: an object's index properties are read, a number's are
        // undefined, and null throws once a pair is replaced
        let text = |quotes: Value, src: &str| {
            let mut options = serde_json::Map::new();
            options.insert("typographer".into(), Value::Bool(true));
            options.insert("quotes".into(), quotes);
            let md = MarkdownIt::with_options("default", &options).unwrap();
            let tokens = md.parse_inline(src, &mut Env::default());
            tokens[0].children.as_ref().unwrap()[0].content.clone()
        };
        let object =
            serde_json::json!({"0": "\u{ab}", "1": [1, [2, null]], "2": null, "3": {"a": 1}});
        assert_eq!(
            text(object, "\"a\" 'b'"),
            "\u{ab}a1,2, nullb[object Object]"
        );
        let partial = serde_json::json!({"1": "R", "3": 7});
        assert_eq!(text(partial, "\"a\" 'b'"), "undefinedaR undefinedb7");
        assert_eq!(text(Value::from(5), "\"a\""), "undefinedaundefined");
        // null: an apostrophe does not read quotes, a pair does, and throws (node:
        // `TypeError: Cannot read properties of null (reading '0')`, '2' for single quotes)
        assert_eq!(text(Value::Null, "don't"), "don\u{2019}t");
        let thrown = |src: &str| {
            let mut options = serde_json::Map::new();
            options.insert("typographer".into(), Value::Bool(true));
            options.insert("quotes".into(), Value::Null);
            let md = MarkdownIt::with_options("default", &options).unwrap();
            md.try_parse(src, &mut Env::default()).unwrap_err()
        };
        let null =
            |i: u8| JsError::type_error(format!("Cannot read properties of null (reading '{i}')"));
        assert_eq!(thrown("\"a\""), null(0));
        assert_eq!(thrown("'a' \"b\""), null(2));
        assert_eq!(thrown("x\n\n> \"a\" 'b'"), null(0));
    }

    #[test]
    fn validate_link_matches_markdown_it() {
        let md = MarkdownIt::default();
        for (url, ok) in [
            ("http://x", true),
            ("javascript:alert(1)", false),
            ("  JavaScript:x", false),
            ("vbscript:x", false),
            ("file:///etc", false),
            ("data:text/html,x", false),
            ("data:image/png;base64,x", true),
            ("DATA:IMAGE/WEBP;x", true),
            ("data:image/svg+xml;x", false),
            ("javascript", true),
        ] {
            assert_eq!(md.validate_link(&s(url)), ok, "{url}");
        }
    }

    /// `md.parse(src)`'s inline children as `type content` (and a link's href), with the
    /// options given as JSON.
    fn children(options: &str, src: &str) -> Vec<String> {
        let options: serde_json::Map<String, Value> = serde_json::from_str(options).unwrap();
        let md = MarkdownIt::with_options("default", &options).unwrap();
        let tokens = md.parse(src, &mut Env::default());
        let inline = tokens.iter().find(|t| t.kind == "inline").unwrap();
        inline
            .children
            .as_ref()
            .unwrap()
            .iter()
            .map(|t| match t.attr_get("href") {
                Some(href) => format!("{} {}", t.kind, href.to_js_string()),
                None => format!("{} {}", t.kind, t.content),
            })
            .collect()
    }

    #[test]
    fn lone_surrogates_the_engine_makes_are_u_fffd_and_halves_that_meet_pair_again() {
        // node (upstream's markdown-it): node's strings hold lone surrogates where the port's
        // have U+FFFD, which is what node writes encoding them as UTF-8; where two halves end
        // up side by side in one string, they are a pair again in both.
        let quotes = r#"{"typographer": true, "quotes": "😀😀😀😀"}"#;
        // `quotes[i]` is one code unit, half of a pair: an empty quoted pair is the emoji
        assert_eq!(
            children(quotes, "\"\" '' \"a\" 'b'"),
            ["text 😀 😀 \u{FFFD}a\u{FFFD} \u{FFFD}b\u{FFFD}"]
        );
        // an `xn--` label decoding to a surrogate: alone (zc0c), after a letter (a-8f4g), and
        // two of them side by side (8c9bk9h: U+D83D, U+DE00), in autolinks and linkify links
        let links = |options: &str, src: &str| {
            assert_eq!(
                children(options, src),
                [
                    "link_open http://xn--Zc0c.com",
                    "text http://\u{FFFD}.com",
                    "link_close ",
                    "text  ",
                    "link_open http://xn--8c9bk9h.com",
                    "text http://😀.com",
                    "link_close ",
                    "text  ",
                    "link_open http://xn--a-8f4g.com",
                    "text http://a\u{FFFD}.com",
                    "link_close ",
                ],
                "{src}"
            );
        };
        links(
            "{}",
            "<http://xn--Zc0c.com> <http://xn--8c9bk9h.com> <http://xn--a-8f4g.com>",
        );
        links(
            r#"{"linkify": true}"#,
            "http://xn--Zc0c.com http://xn--8c9bk9h.com http://xn--a-8f4g.com",
        );
        let md = MarkdownIt::default();
        let text = |url: &str| js::to_string(&md.normalize_link_text(&s(url)));
        assert_eq!(text("http://xn--8c9bk9h.com"), "http://😀.com");
        assert_eq!(text("http://xn--zc0c.com"), "http://\u{FFFD}.com");
        assert_eq!(text("xn--8c9bk9h.xn--zc0c"), "xn--8c9bk9h.xn--zc0c");
        // linkify-it's `maxLength` (10,000 past the schema) cuts the pair: the link text ends
        // with its high half, the text after it starts with its low half, and the href
        // encodes the high half as U+FFFD (node: '...a\ud83d', '\ude00z')
        let a = "a".repeat(9991);
        let cut = format!("http://a.com/{a}😀z");
        assert_eq!(
            children(r#"{"linkify": true}"#, &cut),
            [
                format!("link_open http://a.com/{a}%EF%BF%BD"),
                format!("text http://a.com/{a}\u{FFFD}"),
                "link_close ".to_owned(),
                "text \u{FFFD}z".to_owned(),
            ]
        );
    }

    /// A token's children as node's `JSON.stringify` shows their strings: a lone surrogate
    /// the engine made as `\\udxxx` (the exact UTF-16 the rules read back).
    fn exact_children(options: &str, src: &str) -> Vec<String> {
        let options: serde_json::Map<String, Value> = serde_json::from_str(options).unwrap();
        let md = MarkdownIt::with_options("default", &options).unwrap();
        let tokens = md.parse(src, &mut Env::default());
        let inline = tokens.iter().find(|t| t.kind == "inline").unwrap();
        let escaped = |units: &[u16]| -> String {
            char::decode_utf16(units.iter().copied())
                .map(|c| match c {
                    Ok(c) => c.to_string(),
                    Err(e) => format!("\\u{:04x}", e.unpaired_surrogate()),
                })
                .collect()
        };
        inline
            .children
            .as_ref()
            .unwrap()
            .iter()
            .map(|t| match t.attr_get("href") {
                Some(href) => format!("{} {}", t.kind, href.to_js_string()),
                None => format!("{} {}", t.kind, escaped(&t.content_utf16())),
            })
            .collect()
    }

    #[test]
    fn a_lone_surrogate_the_engine_made_reads_back_as_node_reads_it() {
        // node (upstream's markdown-it 15.0.1 in Markdoc 0.5.11's Tokenizer; work/utf16/kids.cjs):
        // smartquotes reads the code unit next to a quote, in its own token or the nearest one
        // with content before or after it; replacements and the joins of adjacent text tokens
        // read whole contents. Where the engine made a lone surrogate there (an `xn--` label
        // that decodes to one, linkify-it's `maxLength` cutting a pair, astral `quotes`), it
        // is neither punctuation nor space for smartquotes, and the port reads that code unit
        // too (the token keeps its exact UTF-16 beside the lossy `content`).
        let a = "a".repeat(9991);
        let cases: &[(&str, &str, &[&str])] = &[
            (
                r#"{"linkify": true, "typographer": true}"#,
                "http://xn--Zc0c\"a\"",
                &[
                    "link_open http://xn--Zc0c",
                    "text http://\\udcfd",
                    "link_close ",
                    "text \"a\"",
                ],
            ),
            (
                r#"{"typographer": true}"#,
                "<http://xn--Zc0c\"a\">",
                &[
                    "link_open http://xn--Zc0c%22a%22",
                    "text http://\\udcfd\"a\"",
                    "link_close ",
                ],
            ),
            (
                r#"{"typographer": true}"#,
                "\"b <http://xn--Zc0c>\"c\"",
                &[
                    "text “b ",
                    "link_open http://xn--Zc0c",
                    "text http://\\udcfd",
                    "link_close ",
                    "text \"c”",
                ],
            ),
            (
                r#"{"linkify": true, "typographer": true}"#,
                "http://a.com/{a}😀\"x\"",
                &[
                    "link_open http://a.com/{a}%EF%BF%BD",
                    "text http://a.com/{a}\\ud83d",
                    "link_close ",
                    "text \\ude00\"x\"",
                ],
            ),
            (
                r#"{"typographer": true, "quotes": "😀😀😀😀"}"#,
                "\"\" '' \"a\" 'b'",
                &["text 😀 😀 \\ud83da\\ude00 \\ud83db\\ude00"],
            ),
            (
                r#"{"typographer": true, "quotes": "😀😁😂🤣"}"#,
                "\"*a*\" '_b_' \"`c`\" x\"y\" \"\\\"\" \"<http://xn--Zc0c>\"",
                &[
                    "text \\ud83d",
                    "em_open ",
                    "text a",
                    "em_close ",
                    "text \\ude00 \\ud83d",
                    "em_open ",
                    "text b",
                    "em_close ",
                    "text \\ude01 \\ud83d",
                    "code_inline c",
                    "text \\ude00 x\"y\" \\ud83d\"\\ude00 \\ud83d",
                    "link_open http://xn--Zc0c",
                    "text http://\\udcfd",
                    "link_close ",
                    "text \\ude00",
                ],
            ),
            (
                r#"{"typographer": true, "linkify": true, "quotes": "😀😁😂🤣"}"#,
                "\"http://xn--Zc0c\" (c)http://xn--Zc0c.com'x' 'http://a.com/{a}😀'",
                &[
                    "text \\ud83d",
                    "link_open http://xn--Zc0c",
                    "text http://\\udcfd",
                    "link_close ",
                    "text \\ude00 ©",
                    "link_open http://xn--Zc0c.com",
                    "text http://\\udcfd.com",
                    "link_close ",
                    "text ’x’ \\ud83d",
                    "link_open http://a.com/{a}%EF%BF%BD",
                    "text http://a.com/{a}\\ud83d",
                    "link_close ",
                    "text \\ude00\\ude01",
                ],
            ),
        ];
        for (options, src, want) in cases {
            let src = src.replace("{a}", &a);
            let want: Vec<String> = want.iter().map(|w| w.replace("{a}", &a)).collect();
            assert_eq!(exact_children(options, &src), want, "{options} {src:?}");
            // `content` itself is the lossy form: U+FFFD for each lone surrogate
            let lossy: Vec<String> = want
                .iter()
                .map(|w| {
                    let mut out = w.clone();
                    for hex in ["d83d", "dcfd", "de00", "de01"] {
                        out = out.replace(&format!("\\u{hex}"), "\u{FFFD}");
                    }
                    out
                })
                .collect();
            assert_eq!(children(options, &src), lossy, "{options} {src:?}");
        }
    }
}
