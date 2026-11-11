//! Port of upstream Markdoc v0.5.11's `src/tokenizer/index.ts`.
//!
//! Owner: the plugins agent.
//!
//! The `Tokenizer`: markdown-it with Markdoc's plugins, `lheading` and `code` disabled, and
//! `TokenizerOptions` whose defaults are upstream's (`new Tokenizer()`: everything off).
//!
//! ```
//! use markdoc::next::tokenizer::{Tokenizer, TokenizerOptions};
//!
//! let tokenizer = Tokenizer::new(TokenizerOptions {
//!     allow_indentation: true,
//!     allow_comments: true,
//!     ..TokenizerOptions::default()
//! });
//! let tokens = tokenizer.tokenize("{% note %}\nHello\n{% /note %}\n").unwrap();
//! let kinds: Vec<&str> = tokens.iter().map(|t| t.kind.as_str()).collect();
//! assert_eq!(
//!     kinds,
//!     ["tag_open", "paragraph_open", "inline", "paragraph_close", "tag_close"]
//! );
//! ```
//!
//! The tokens are markdown-it's ([`Token`]), with what Markdoc's plugins add: a tag's `meta`
//! (the grammar's result as JSON, keys in upstream's order), `errors`, and the `position` of
//! a tag inside a fence. Every position is a UTF-16 index, as upstream's are.
//!
//! # Divergences from upstream
//!
//! - **Recursion depth.** A tag's values and markdown-it's own nesting (with a raised
//!   `maxNesting`) share node's stack as upstream's do ([`crate::depth`]: each level's
//!   [`crate::grammar::cost`] or [`crate::markdown_it::cost`]): `tokenize` returns V8's
//!   `RangeError: Maximum call stack size exceeded` where a fresh node 22 process throws,
//!   called from the top level of a script (4,074 nested arrays, 2,401 hashes, 2,353 calls;
//!   see the engine's module docs for its constructs); a warmed-up node goes deeper. Where
//!   the stack cannot grow ([`crate::depth::can_grow`]), at
//!   [`crate::grammar::MAX_NESTING`] (300) levels of values and
//!   [`crate::markdown_it::MAX_DEPTH`] (160) nested calls of the engine instead, the host's
//!   limit there. Tests:
//!   `tests::values_nested_too_deep_throw_a_range_error_wherever_the_tag_is`, `stack`.
//! - **Lone surrogates.** The grammar's syntax error quotes the one UTF-16 code unit it
//!   failed at, which can be half of an astral character (`{% 😀 %}`: `... but "\ud83d"
//!   found.`); a token's `meta` is JSON, whose strings are Rust strings, so the port's message
//!   has U+FFFD there, the bytes node writes encoding it as UTF-8. The engine's own lone
//!   surrogates are in [`crate::markdown_it`]'s docs. Test:
//!   `tests::a_grammar_error_quoting_half_an_astral_character_has_u_fffd`.

pub mod plugins;
pub mod utils;

use serde_json::{Map, Value};

use crate::grammar::RangeError;
use crate::markdown_it::{Env, JsError, MarkdownIt, Token};

use plugins::{annotations, comments, frontmatter, link};

/// `LinkPluginOptions`: `{validatedProtocols: string[]}`, or what upstream makes of any other
/// value given as `linkValidationOptions` (see [`ValidatedProtocols`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkPluginOptions {
    /// `options.validatedProtocols`: the protocols (without `://`) after whose URLs a tag
    /// is an `href-format-invalid` error.
    pub validated_protocols: ValidatedProtocols,
}

/// `options.validatedProtocols` as `containsMarkdocTagInUrl` uses it: `for (const protocol of
/// allowedProtocols)`, each through `${protocol}://`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidatedProtocols {
    /// An iterable: the text of each item, an array's elements through `String()` (`null`
    /// is `"null"`, `[1, 2]` is `"1,2"`, an object `"[object Object]"`), a string's code
    /// points.
    Iterable(Vec<String>),
    /// Anything else, `undefined` included (options that are not an object, or lack the
    /// key): the first inline token holding a closed tag throws `TypeError: allowedProtocols
    /// is not iterable`.
    NotIterable,
}

impl LinkPluginOptions {
    /// `{validatedProtocols: protocols}`.
    pub fn new<S: Into<String>>(protocols: impl IntoIterator<Item = S>) -> Self {
        LinkPluginOptions {
            validated_protocols: ValidatedProtocols::Iterable(
                protocols.into_iter().map(Into::into).collect(),
            ),
        }
    }

    /// What the link plugin reads from `linkValidationOptions` when it is this JS value
    /// (not `null`: upstream then uses its default).
    pub fn from_json(value: &Value) -> Self {
        let validated_protocols = match value.get("validatedProtocols") {
            // `value.validatedProtocols` of an array or a primitive is undefined too
            Some(Value::Array(items)) if value.is_object() => {
                ValidatedProtocols::Iterable(items.iter().map(js_string).collect())
            }
            Some(Value::String(s)) if value.is_object() => {
                ValidatedProtocols::Iterable(s.chars().map(String::from).collect())
            }
            _ => ValidatedProtocols::NotIterable,
        };
        LinkPluginOptions {
            validated_protocols,
        }
    }
}

impl Default for LinkPluginOptions {
    /// What `new Tokenizer({allowLinkValidation: true})` uses: `['http', 'https']`.
    fn default() -> Self {
        LinkPluginOptions::new(["http", "https"])
    }
}

/// `String(value)` of a JSON value.
fn js_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            crate::markdown_it::js::number_to_string(n.as_f64().unwrap_or(f64::NAN))
        }
        // Array.prototype.join: null and undefined items are ''
        Value::Array(items) => items
            .iter()
            .map(|x| {
                if x.is_null() {
                    String::new()
                } else {
                    js_string(x)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// The Tokenizer's config: `MarkdownIt.MarkdownItOptions & {allowIndentation?, allowComments?,
/// allowLinkValidation?, linkValidationOptions?}`. The default is `new Tokenizer()`'s: every
/// switch off, markdown-it's own defaults.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TokenizerOptions {
    /// markdown-it's four-column indentation checks are skipped (Markdoc's patch).
    pub allow_indentation: bool,
    /// `<!-- ... -->` is a `comment` token (the comments plugin).
    pub allow_comments: bool,
    /// An inline token with a tag right after a URL gets an `href-format-invalid` error (the
    /// link plugin).
    pub allow_link_validation: bool,
    /// The link plugin's options (`linkValidationOptions`); `None` is upstream's default,
    /// `['http', 'https']`. Read only when `allow_link_validation` is set, as upstream reads
    /// them.
    pub link_validation_options: Option<LinkPluginOptions>,
    /// markdown-it's own options in their JavaScript names (`html`, `xhtmlOut`, `breaks`,
    /// `langPrefix`, `linkify`, `typographer`, `quotes`, `maxNesting`), applied as
    /// `new MarkdownIt(config)` applies its config (`Object.assign` onto the default preset's
    /// options; see [`crate::markdown_it::Options::assign`]): any JSON value, read as JS reads
    /// it.
    pub markdown_it: Map<String, Value>,
}

impl TokenizerOptions {
    /// Reads a config object (JSON) as upstream's constructor reads its config:
    /// `allowIndentation`, `allowComments` and `allowLinkValidation` by JavaScript
    /// truthiness, `linkValidationOptions ?? default` as whatever JS value it is (see
    /// [`LinkPluginOptions::from_json`]; it is only used with `allowLinkValidation`), and
    /// every other key as one of markdown-it's options. It never fails: every JSON config is
    /// a JS config; what upstream throws for one, `tokenize` returns.
    ///
    /// One unknown key matters: `__proto__`. `new MarkdownIt(config)` copies the config onto
    /// its options with `Object.assign`, which sets their prototype to an object given under
    /// `__proto__`; markdown-it's own options are the options' own properties, but Markdoc's
    /// `allowIndentation` is then inherited from that object when the config does not give
    /// it.
    pub fn from_json(config: &Map<String, Value>) -> Result<TokenizerOptions, String> {
        let mut options = TokenizerOptions::default();
        if !config.contains_key("allowIndentation")
            && let Some(Value::Object(proto)) = config.get("__proto__")
        {
            options.allow_indentation = proto.get("allowIndentation").is_some_and(truthy);
        }
        for (key, value) in config {
            match key.as_str() {
                "allowIndentation" => options.allow_indentation = truthy(value),
                "allowComments" => options.allow_comments = truthy(value),
                "allowLinkValidation" => options.allow_link_validation = truthy(value),
                "linkValidationOptions" => {
                    // `config.linkValidationOptions ?? default`
                    options.link_validation_options =
                        (!value.is_null()).then(|| LinkPluginOptions::from_json(value));
                }
                _ => {
                    options.markdown_it.insert(key.clone(), value.clone());
                }
            }
        }
        Ok(options)
    }

    /// The object upstream passes to `new MarkdownIt(config)`: the whole config.
    fn markdown_it_config(&self) -> Map<String, Value> {
        let mut config = self.markdown_it.clone();
        config.insert("allowIndentation".into(), self.allow_indentation.into());
        config.insert("allowComments".into(), self.allow_comments.into());
        config.insert(
            "allowLinkValidation".into(),
            self.allow_link_validation.into(),
        );
        if let Some(options) = &self.link_validation_options {
            let mut o = Map::new();
            if let ValidatedProtocols::Iterable(protocols) = &options.validated_protocols {
                o.insert("validatedProtocols".into(), protocols.clone().into());
            }
            config.insert("linkValidationOptions".into(), Value::Object(o));
        }
        config
    }
}

/// JavaScript's ToBoolean of a JSON value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// What upstream's `tokenize` throws: an exception other than the grammar's SyntaxError,
/// which Markdoc's plugins do not catch, with upstream's name and message. These can happen:
///
/// - `RangeError: Maximum call stack size exceeded`: a tag's values, or, with a `maxNesting`
///   far above markdown-it's default, blockquotes, list items, links, images or brackets nest
///   deeper than node's stack takes them (see [`crate::depth`]; where the stack cannot grow,
///   past [`crate::grammar::MAX_NESTING`] and [`crate::markdown_it::MAX_DEPTH`]).
/// - `Error: inline rule didn't increment state.pos`: with `allowComments`, a comment that
///   the scan of a link label reaches (`[<!-- c -->](u)`, or `see [1 <!-- note -->` with no
///   `]` after it): markdown-it's `skipToken` rejects the comment rule's answer.
/// - `TypeError: Cannot read properties of undefined (reading 'length')` (or `'push'`): a
///   closing tag inside a link's text (`[{% /a %}](u)`) leaves markdown-it's delimiter list
///   undefined, and the next rule to use it throws.
/// - `TypeError: Cannot read properties of null (reading '0')` (or `'2'`): `typographer`
///   with `quotes: null`, once a pair of double (single) quotes is replaced.
/// - `TypeError: allowedProtocols is not iterable`: link validation with
///   `linkValidationOptions` whose `validatedProtocols` is not an array or a string, once an
///   inline token holding a closed tag is checked.
///
/// [`Tokenizer::tokenize`] returns these, and never panics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenizeError {
    /// The JavaScript error's `name`: `RangeError`, `Error` or `TypeError`.
    pub name: String,
    /// Its `message`.
    pub message: String,
}

impl std::fmt::Display for TokenizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.message)
    }
}

impl std::error::Error for TokenizeError {}

impl From<JsError> for TokenizeError {
    fn from(error: JsError) -> Self {
        TokenizeError {
            name: error.name,
            message: error.message,
        }
    }
}

impl From<TokenizeError> for JsError {
    fn from(error: TokenizeError) -> Self {
        JsError {
            name: error.name,
            message: error.message,
        }
    }
}

/// Records `error` as thrown on the environment, unless something was thrown already (the
/// first throw is the one that escapes). A rule has no way to unwind markdown-it, so it
/// records the error and returns; the engine then stops (see [`JsError`]), and
/// [`Tokenizer::tokenize`] returns the error and drops the tokens. Whoever runs the plugins on
/// a markdown-it of their own finds it in `env.thrown`.
pub(crate) fn throw(env: &mut Env, error: TokenizeError) {
    env.throw(error.into());
}

/// Whether something was thrown.
pub(crate) fn thrown(env: &Env) -> bool {
    env.has_thrown()
}

/// The grammar's RangeError as what `tokenize` throws.
pub(crate) fn range_error(error: &RangeError) -> TokenizeError {
    TokenizeError {
        name: "RangeError".to_owned(),
        message: error.0.clone(),
    }
}

/// `Tokenizer`
#[derive(Debug, Clone)]
pub struct Tokenizer {
    parser: MarkdownIt,
}

impl Default for Tokenizer {
    /// `new Tokenizer()`
    fn default() -> Self {
        Tokenizer::new(TokenizerOptions::default())
    }
}

impl Tokenizer {
    /// `new Tokenizer(config)`
    pub fn new(config: TokenizerOptions) -> Tokenizer {
        const BUILT_IN: &str = "the default preset has the rules Markdoc's plugins insert before";
        let mut parser = MarkdownIt::with_options("default", &config.markdown_it_config())
            .expect("the default preset exists");
        annotations::plugin(&mut parser).expect(BUILT_IN);
        frontmatter::plugin(&mut parser).expect(BUILT_IN);

        parser
            .disable(
                &[
                    "lheading",
                    // Disable indented `code_block` support https://spec.commonmark.org/0.30/#indented-code-block
                    "code",
                ],
                false,
            )
            .expect("the default preset has lheading and code");

        if config.allow_comments {
            comments::plugin(&mut parser).expect(BUILT_IN);
        }
        if config.allow_link_validation {
            // Set http and https as the default protocols to validate
            link::plugin(
                &mut parser,
                &config.link_validation_options.unwrap_or_default(),
            );
        }
        Tokenizer { parser }
    }

    /// `tokenize(content)`: the token stream, or what upstream throws (see
    /// [`TokenizeError`]).
    pub fn tokenize(&self, content: &str) -> Result<Vec<Token>, TokenizeError> {
        let mut env = Env::default();
        self.parser
            .try_parse(content, &mut env)
            .map_err(TokenizeError::from)
    }

    /// The markdown-it instance (upstream's `private parser`).
    pub fn parser(&self) -> &MarkdownIt {
        &self.parser
    }
}

#[cfg(test)]
mod stack;
#[cfg(test)]
mod tests;
