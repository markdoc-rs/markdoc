//! E1: parse switches. One value for everything that decides how a text becomes an AST.
//!
//! [`ParseOptions`] holds upstream's tokenizer options ([`TokenizerOptions`]: `allow_indentation`,
//! `allow_comments`, markdown-it's own such as `typographer`) and parser arguments
//! ([`ParserArgs`]), plus this crate's two switches, both off by default as upstream has
//! neither:
//!
//! - [`ParseOptions::strip_bom`]: a byte-order mark that starts the text is not tokenized
//!   (upstream keeps it, so a heading on line 1 is then a paragraph); positions still point
//!   into the text given.
//! - [`ParseOptions::frontmatter`]: [`Frontmatter::Yaml`] makes the document's `frontmatter`
//!   attribute the YAML parsed into a [`crate::types::Scalar::Object`], keys in YAML order.
//!   YAML that does not parse to a mapping keeps upstream's raw string and adds a
//!   `frontmatter-invalid` error to the document; parsing never fails over it.
//!
//! [`parse()`] with [`ParseOptions::default()`] is upstream's `Markdoc.parse(content)`.
//! Tools that must agree on what counts as a tag (a linter, an editor, a renderer) share one
//! `ParseOptions`, read from the `[parse]` table of a schema overlay ([`ParseOptions::from_toml`]).
//!
//! This crate's own positions ([`crate::ast::Node::span`], and the errors' `span`) are the
//! parser's switch [`ParserArgs::spans`], off by default; a tool that reports positions turns
//! it on with [`ParseOptions::with_spans`] (or `options.args.spans = true`). It is each tool's
//! choice, not the corpus's, so the `[parse]` table has no key for it.
//!
//! ```
//! use markdoc::ext::{Frontmatter, ParseOptions, parse};
//! use markdoc::types::Scalar;
//!
//! let options = ParseOptions::from_toml(
//!     "[parse]\nstrip_bom = true\nfrontmatter = \"yaml\"\nallow_indentation = true\n",
//! )
//! .unwrap();
//! assert!(options.strip_bom && options.tokenizer.allow_indentation);
//! assert_eq!(options.frontmatter, Frontmatter::Yaml);
//!
//! let source = "---\ntitle: Hello\n---\n# Hello\n";
//! let doc = parse(source, &options).unwrap();
//! let Some(Scalar::Object(fm)) = doc.attributes.get("frontmatter") else { panic!() };
//! assert_eq!(fm.get("title"), Some(&Scalar::String("Hello".into())));
//! // Off by default: upstream's raw text.
//! let doc = parse(source, &ParseOptions::default()).unwrap();
//! assert_eq!(doc.attributes["frontmatter"], Scalar::String("title: Hello".into()));
//!
//! // A byte-order mark keeps line 1 from being a heading, unless it is stripped.
//! let bom = "\u{FEFF}# Hello\n";
//! assert_eq!(parse(bom, &options).unwrap().children[0].node_type.as_str(), "heading");
//! let doc = parse(bom, &ParseOptions::default()).unwrap();
//! assert_eq!(doc.children[0].node_type.as_str(), "paragraph");
//! ```

use crate::ast::Node;
use crate::next::index;
use crate::next::parser::ParserArgs;
use crate::next::tokenizer::{Tokenizer, TokenizerOptions};
use crate::types::{MarkdocError, Result};
use serde_json::Value as Json;
use std::borrow::Cow;
use std::sync::{Arc, Mutex};

/// What the document's `frontmatter` attribute holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Frontmatter {
    /// Upstream's: the raw text between the `---` lines.
    #[default]
    Raw,
    /// The YAML parsed into an object; YAML that is no mapping keeps the raw text and adds a
    /// `frontmatter-invalid` error (see the module's documentation).
    Yaml,
}

/// How [`parse()`] reads a text: the tokenizer's options, the parser's arguments and this
/// crate's two switches. The default is upstream's `Markdoc.parse(content)`.
///
/// `strip_bom` and `frontmatter` set the parser arguments' own switches
/// ([`ParserArgs::strip_bom`], [`ParserArgs::parse_frontmatter`]); either place turns one on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParseOptions {
    /// `new Tokenizer(config)`: `allow_indentation`, `allow_comments`, link validation, and
    /// markdown-it's options (`typographer` and the rest) in their JavaScript names.
    pub tokenizer: TokenizerOptions,
    /// `parse(tokens, args)`: the file name, slots, locations, conditional tags, and this
    /// crate's spans ([`ParserArgs::spans`], off by default; see [`ParseOptions::with_spans`]).
    pub args: ParserArgs,
    /// Leave a byte-order mark that starts the text out of tokenizing.
    pub strip_bom: bool,
    /// What the document's `frontmatter` attribute holds.
    pub frontmatter: Frontmatter,
}

impl ParseOptions {
    /// The options of a TOML document's `[parse]` table (a schema overlay's, for instance);
    /// the defaults when it has none. Keys, all optional:
    ///
    /// | Key | Type | Sets |
    /// |---|---|---|
    /// | `allow_indentation` | boolean | [`TokenizerOptions::allow_indentation`] |
    /// | `allow_comments` | boolean | [`TokenizerOptions::allow_comments`] |
    /// | `allow_link_validation` | boolean | [`TokenizerOptions::allow_link_validation`] |
    /// | `typographer` | boolean | markdown-it's `typographer` |
    /// | `strip_bom` | boolean | [`ParseOptions::strip_bom`] |
    /// | `frontmatter` | `"raw"` or `"yaml"` | [`ParseOptions::frontmatter`] |
    /// | `slots` | boolean | [`ParserArgs::slots`] |
    ///
    /// A key of another name or type is an error, so a misspelt switch is not silently off.
    pub fn from_toml(text: &str) -> Result<ParseOptions> {
        let doc: toml::Table = toml::from_str(text)
            .map_err(|e| MarkdocError::ParseError(format!("parse options: {e}")))?;
        let mut options = ParseOptions::default();
        if let Some(table) = doc.get("parse") {
            options.apply_toml(table, "parse")?;
        }
        Ok(options)
    }

    /// Applies the keys of a `[parse]`-shaped TOML table (see [`ParseOptions::from_toml`]) on
    /// top of these options; `name` names the table in errors.
    pub(crate) fn apply_toml(&mut self, table: &toml::Value, name: &str) -> Result<()> {
        let err = |msg: String| MarkdocError::ParseError(format!("[{name}]: {msg}"));
        let table = table
            .as_table()
            .ok_or_else(|| err("expected a table".into()))?;
        for (key, value) in table {
            let flag = || {
                value
                    .as_bool()
                    .ok_or_else(|| err(format!("`{key}` must be a boolean")))
            };
            match key.as_str() {
                "allow_indentation" => self.tokenizer.allow_indentation = flag()?,
                "allow_comments" => self.tokenizer.allow_comments = flag()?,
                "allow_link_validation" => self.tokenizer.allow_link_validation = flag()?,
                "typographer" => {
                    let on = flag()?;
                    self.tokenizer
                        .markdown_it
                        .insert("typographer".into(), Json::Bool(on));
                }
                "strip_bom" => self.strip_bom = flag()?,
                "slots" => self.args.slots = flag()?,
                "frontmatter" => {
                    self.frontmatter = match value.as_str() {
                        Some("raw") => Frontmatter::Raw,
                        Some("yaml") => Frontmatter::Yaml,
                        _ => return Err(err("`frontmatter` must be \"raw\" or \"yaml\"".into())),
                    }
                }
                _ => return Err(err(format!("unknown key `{key}`"))),
            }
        }
        Ok(())
    }

    /// These options with the parser's `file` set (`location.file` of every node), as a
    /// partial is parsed with the options of the document that includes it.
    pub fn with_file(&self, file: impl Into<String>) -> ParseOptions {
        let mut options = self.clone();
        options.args.file = Some(file.into());
        options
    }

    /// These options with this crate's own positions on or off ([`ParserArgs::spans`]: every
    /// node's [`crate::ast::Node::span`] and the parser's and `validate()`'s errors' `span`).
    /// Off by default, as upstream has none; a linter or a language server turns them on.
    ///
    /// ```
    /// use markdoc::ext::{ParseOptions, parse};
    ///
    /// let options = ParseOptions::default().with_spans(true);
    /// let doc = parse("# Hello\n", &options).unwrap();
    /// assert_eq!(doc.children[0].span.as_ref().unwrap().start.line, 1);
    /// assert!(parse("# Hello\n", &ParseOptions::default()).unwrap().children[0].span.is_none());
    /// ```
    pub fn with_spans(&self, spans: bool) -> ParseOptions {
        let mut options = self.clone();
        options.args.spans = spans;
        options
    }

    /// The parser arguments [`parse()`] passes: [`ParseOptions::args`] with the two switches
    /// applied.
    pub fn parser_args(&self) -> Cow<'_, ParserArgs> {
        let yaml = self.frontmatter == Frontmatter::Yaml;
        if (self.strip_bom && !self.args.strip_bom) || (yaml && !self.args.parse_frontmatter) {
            let mut args = self.args.clone();
            args.strip_bom |= self.strip_bom;
            args.parse_frontmatter |= yaml;
            Cow::Owned(args)
        } else {
            Cow::Borrowed(&self.args)
        }
    }

    /// The tokenizer these options describe. Building one is not free, so [`parse()`] keeps
    /// the last few it built; this returns a shared one when it has it.
    pub fn tokenizer(&self) -> Arc<Tokenizer> {
        cached_tokenizer(&self.tokenizer)
    }
}

/// `Markdoc.parse(source, args)` with a tokenizer of `options`: the AST of `source`. An `Err`
/// is where upstream throws (see [`crate::next::index`]).
pub fn parse(source: &str, options: &ParseOptions) -> Result<Node> {
    let args = options.parser_args();
    if options.tokenizer == TokenizerOptions::default() {
        return index::parse_with(index::default_tokenizer(), source, &args);
    }
    index::parse_with(&options.tokenizer(), source, &args)
}

/// How many tokenizers [`cached_tokenizer`] keeps (a corpus's tools use one or two sets of
/// options; a long-running server may see a few more).
const CACHED_TOKENIZERS: usize = 8;

/// A tokenizer for `options`, shared with earlier calls that had the same options.
fn cached_tokenizer(options: &TokenizerOptions) -> Arc<Tokenizer> {
    static CACHE: Mutex<Vec<(TokenizerOptions, Arc<Tokenizer>)>> = Mutex::new(Vec::new());
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(i) = cache.iter().position(|(o, _)| o == options) {
        // Most recently used last.
        let hit = cache.remove(i);
        let tokenizer = Arc::clone(&hit.1);
        cache.push(hit);
        return tokenizer;
    }
    let tokenizer = Arc::new(Tokenizer::new(options.clone()));
    if cache.len() == CACHED_TOKENIZERS {
        cache.remove(0);
    }
    cache.push((options.clone(), Arc::clone(&tokenizer)));
    tokenizer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Scalar;

    fn ast(source: &str, options: &ParseOptions) -> serde_json::Value {
        serde_json::to_value(parse(source, options).unwrap()).unwrap()
    }

    #[test]
    fn the_default_is_upstreams_parse() {
        // Off: the same AST as `Markdoc.parse(content)`, for text each switch would change.
        for source in [
            "\u{FEFF}# Title\n",
            "---\ntitle: x\n---\n# Doc\n",
            "<!-- note -->\nText \"quoted\" -- (c)\n",
            "    {% callout %}\n    indented\n    {% /callout %}\n",
        ] {
            let upstream = index::parse(source, &ParserArgs::default()).unwrap();
            assert_eq!(
                ast(source, &ParseOptions::default()),
                serde_json::to_value(upstream).unwrap(),
                "{source:?}"
            );
        }
    }

    #[test]
    fn each_switch_changes_only_its_own_part() {
        let on = |f: fn(&mut ParseOptions)| {
            let mut options = ParseOptions::default();
            f(&mut options);
            options
        };
        let first = |source: &str, options: &ParseOptions| {
            parse(source, options).unwrap().children[0]
                .node_type
                .as_str()
                .to_string()
        };
        // Indentation: a list indented four columns is a list, not paragraph text.
        let indented = "Text\n\n    - a\n    - b\n";
        let second = |options: &ParseOptions| {
            parse(indented, options).unwrap().children[1]
                .node_type
                .as_str()
                .to_string()
        };
        assert_eq!(second(&ParseOptions::default()), "paragraph");
        assert_eq!(
            second(&on(|o| o.tokenizer.allow_indentation = true)),
            "list"
        );
        // Comments: a comment node instead of text.
        let comment = "<!-- vale off -->\n";
        assert_eq!(
            first(comment, &on(|o| o.tokenizer.allow_comments = true)),
            "comment"
        );
        // The BOM, and the frontmatter as an object.
        assert_eq!(
            first("\u{FEFF}# T\n", &on(|o| o.strip_bom = true)),
            "heading"
        );
        let doc = parse(
            "---\na: 1\n---\n",
            &on(|o| o.frontmatter = Frontmatter::Yaml),
        )
        .unwrap();
        assert!(matches!(doc.attributes["frontmatter"], Scalar::Object(_)));
        // Either place turns a switch on.
        let mut options = ParseOptions::default();
        options.args.strip_bom = true;
        assert_eq!(first("\u{FEFF}# T\n", &options), "heading");
        assert!(matches!(options.parser_args(), Cow::Borrowed(_)));
        options.frontmatter = Frontmatter::Yaml;
        assert!(options.parser_args().parse_frontmatter);
    }

    #[test]
    fn it_reads_frontmatter_as_the_old_parser_did() {
        // The old parser stripped the BOM and parsed the YAML; the switches do both.
        let source = "\u{FEFF}---\ntitle: T\nswVersion: 1.10\nproducts: [Bot250, Bot600]\nnested: {a: 1}\n---\n# T\n";
        let options =
            ParseOptions::from_toml("[parse]\nstrip_bom = true\nfrontmatter = \"yaml\"\n").unwrap();
        let new = parse(source, &options).unwrap();
        // What the old parser (removed at the switch-over) gave for the same source.
        let old = Scalar::Object(indexmap::IndexMap::from([
            ("title".to_string(), Scalar::String("T".into())),
            ("swVersion".to_string(), Scalar::Number(1.1)),
            (
                "products".to_string(),
                Scalar::Array(vec![
                    Scalar::String("Bot250".into()),
                    Scalar::String("Bot600".into()),
                ]),
            ),
            (
                "nested".to_string(),
                Scalar::Object(indexmap::IndexMap::from([(
                    "a".to_string(),
                    Scalar::Number(1.0),
                )])),
            ),
        ]));
        assert_eq!(new.attributes["frontmatter"], old);
        assert_eq!(new.children[0].node_type.as_str(), "heading");
    }

    #[test]
    fn with_file_names_every_node() {
        let options = ParseOptions::default().with_file("partials/p.md");
        let doc = parse("# H\n", &options).unwrap();
        assert_eq!(options.args.file.as_deref(), Some("partials/p.md"));
        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("partials/p.md"), "{json}");
    }

    #[test]
    fn a_tokenizer_is_built_once_per_set_of_options() {
        let mut options = ParseOptions::default();
        options.tokenizer.allow_comments = true;
        let a = options.tokenizer();
        let b = options.clone().tokenizer();
        assert!(Arc::ptr_eq(&a, &b));
        options.tokenizer.allow_indentation = true;
        assert!(!Arc::ptr_eq(&a, &options.tokenizer()));
    }

    #[test]
    fn the_parse_table_is_strict() {
        let err = |text: &str| ParseOptions::from_toml(text).unwrap_err().to_string();
        assert!(err("[parse]\nallow_indentaton = true\n").contains("allow_indentaton"));
        assert!(err("[parse]\nstrip_bom = \"yes\"\n").contains("strip_bom"));
        assert!(err("[parse]\nfrontmatter = \"json\"\n").contains("frontmatter"));
        assert!(err("parse = 1\n").contains("table"));
        assert!(err("[parse\n").contains("parse options"));
        // No table: the defaults; other tables are someone else's.
        assert_eq!(
            ParseOptions::from_toml("[tags.x]\n").unwrap(),
            ParseOptions::default()
        );
        let all = ParseOptions::from_toml(
            "[parse]\nallow_indentation = true\nallow_comments = true\n\
             allow_link_validation = true\ntypographer = true\nstrip_bom = true\n\
             frontmatter = \"raw\"\nslots = true\n",
        )
        .unwrap();
        assert!(all.tokenizer.allow_link_validation && all.args.slots && all.strip_bom);
        assert_eq!(
            all.tokenizer.markdown_it.get("typographer"),
            Some(&Json::Bool(true))
        );
    }
}
