//! Occurrences: every `{% %}` the tokenizer recognised in a document, where it is, and how
//! the parser pairs it (an extension, for editors and linters; upstream has no such API).
//!
//! The AST cannot say this: a closing tag that matched its opening and an annotation make no
//! node, `{% x /%}` and `{% x %}{% /x %}` make the same node, and a tag the tokenizer saw in
//! an image's alt text makes none. An occurrence is read from the tokens instead, by the same
//! tokenizer and the same position finding the parser uses, so what counts as a tag (code
//! spans, escapes, fences, comments, the indentation and comment switches) is exactly what
//! the parser sees:
//!
//! - its kind: [`OccurrenceKind::Open`], [`SelfClosing`](OccurrenceKind::SelfClosing),
//!   [`Close`](OccurrenceKind::Close), [`Annotation`](OccurrenceKind::Annotation) (`{% #id
//!   .class key=value %}` after a block or inline), [`Interpolation`](OccurrenceKind::Interpolation)
//!   (`{% $x %}`, `{% f() %}`), or [`Error`](OccurrenceKind::Error) (a body the tag grammar
//!   rejected, with its message);
//! - its byte span (`{%` to `%}`), and its [`Span`] (1-based lines, UTF-16 columns and
//!   byte offsets, as the parser's `span`s);
//! - whether it is inside a fence (a fence's content is split into text and tags unless the
//!   fence's own tag says `process=false`);
//! - its partner: an opening tag and the closing tag that closed it, each the other's index,
//!   as the parser pairs them (a closing tag closes the node on top of the parser's stack
//!   only, so `{% a %}{% b %}{% /a %}` pairs nothing, and an inline tag left open is never
//!   closed by a block one); `None` for one left open and a closing tag that closed
//!   nothing (the parser's `missing-closing` and `missing-opening`).
//!
//! A tag in a fence's info string (```` ```js {% .x %} ````) gives the fence its attributes:
//! it is an [`Annotation`](OccurrenceKind::Annotation) (or an `Error`, the fence's
//! `fence-tag-error`), before the occurrences in the fence's content. Tags inside an image's
//! alt text are left out: the parser never reads them, and they have no position.
//!
//! ```
//! use markdoc::ext::ParseOptions;
//! use markdoc::ext::occurrences::{OccurrenceKind, occurrences};
//!
//! let options = ParseOptions::from_toml(
//!     "[parse]\nallow_indentation = true\nallow_comments = true\n",
//! )
//! .unwrap();
//! let src = "{% note %}\nSee `{% x %}` and {% $v %}.\n{% /note %}\n";
//! let found = occurrences(src, &options).unwrap();
//! let kinds: Vec<String> = found
//!     .iter()
//!     .map(|o| match &o.kind {
//!         OccurrenceKind::Open { name, .. } => format!("open {name}"),
//!         OccurrenceKind::Close { name } => format!("close {name}"),
//!         OccurrenceKind::Interpolation { .. } => "interpolation".into(),
//!         other => format!("{other:?}"),
//!     })
//!     .collect();
//! // The `{% x %}` in a code span is no tag.
//! assert_eq!(kinds, ["open note", "interpolation", "close note"]);
//! assert_eq!(&src[found[1].span.clone()], "{% $v %}");
//! assert_eq!((found[0].partner, found[2].partner), (Some(2), Some(0)));
//! assert_eq!(found[2].location.start.line, 3);
//! ```

use crate::ast::{self, Node, PathSegment};
use crate::ext::ParseOptions;
use crate::grammar::{self, AttributeType};
use crate::markdown_it::Token;
use crate::next::index::{default_tokenizer, parse_tokens};
use crate::next::parser::ParserArgs;
use crate::next::parser::positions::Positions;
use crate::next::tokenizer::TokenizerOptions;
use crate::positions::LineIndex;
use crate::types::{MarkdocError, Position, Result, Scalar, Span, js_insert};
use indexmap::IndexMap;
use std::ops::Range;

/// One `{% %}` the tokenizer recognised.
#[derive(Debug, Clone)]
pub struct Occurrence {
    pub kind: OccurrenceKind,
    /// Its bytes in the source, `{%` to `%}`.
    pub span: Range<usize>,
    /// The same as the parser's `span`s ([`crate::ast::Node::span`]): 1-based lines, UTF-16
    /// columns, byte offsets, and `file` from the parser arguments.
    pub location: Span,
    /// Inside a fence's content.
    pub in_fence: bool,
    /// The index of the opening tag a closing one closed, or of the closing tag that closed
    /// an opening one, as the parser pairs them.
    pub partner: Option<usize>,
}

/// What an [`Occurrence`] is. Attributes are as the tag wrote them, in order: `primary` for
/// the value after the name, `id` for `#id`, `class` an object of `.name: true`; values are
/// literals, or [`Scalar::Variable`] and [`Scalar::Function`] where they are expressions.
#[derive(Debug, Clone, PartialEq)]
pub enum OccurrenceKind {
    /// `{% name ... %}`
    Open {
        name: String,
        attributes: IndexMap<String, Scalar>,
    },
    /// `{% name ... /%}`
    SelfClosing {
        name: String,
        attributes: IndexMap<String, Scalar>,
    },
    /// `{% /name %}`
    Close { name: String },
    /// `{% #id .class key=value %}`: attributes for the block or inline it follows.
    Annotation {
        attributes: IndexMap<String, Scalar>,
    },
    /// `{% $path %}` or `{% function(...) %}`: a [`Scalar::Variable`] or
    /// [`Scalar::Function`].
    Interpolation { value: Scalar },
    /// A body the tag grammar could not parse: its syntax error's message.
    Error { message: String },
}

/// The tokens of `source` and the parser arguments, as [`crate::ext::parse`] makes and uses
/// them.
fn tokenize(source: &str, options: &ParseOptions) -> Result<(Vec<Token>, ParserArgs)> {
    let args = options.parser_args().into_owned();
    let text = if args.strip_bom {
        source.strip_prefix('\u{FEFF}').unwrap_or(source)
    } else {
        source
    };
    let tokens = if options.tokenizer == TokenizerOptions::default() {
        default_tokenizer().tokenize(text)
    } else {
        options.tokenizer().tokenize(text)
    };
    let tokens = tokens.map_err(|e| MarkdocError::ParseError(e.to_string()))?;
    Ok((tokens, args))
}

/// Every `{% %}` of `source` that the tokenizer `options` describe recognises, in document
/// order (see the module's documentation); `file` and `strip_bom` as the parser has them.
/// `Err` where tokenizing throws.
pub fn occurrences(source: &str, options: &ParseOptions) -> Result<Vec<Occurrence>> {
    let (tokens, args) = tokenize(source, options)?;
    Ok(from_tokens(&tokens, source, &args))
}

/// The AST of `source` (what [`crate::ext::parse`] returns) and its occurrences, from one
/// tokenizing.
pub fn parse_with_occurrences(
    source: &str,
    options: &ParseOptions,
) -> Result<(Node, Vec<Occurrence>)> {
    let (tokens, args) = tokenize(source, options)?;
    let node = parse_tokens(&tokens, source, &args)?;
    Ok((node, from_tokens(&tokens, source, &args)))
}

/// A node on the parser's stack, as far as pairing needs it.
struct Open {
    type_name: String,
    tag: Option<String>,
    occurrence: Option<usize>,
}

/// A token's children being walked.
struct Walk<'t> {
    children: &'t [Token],
    spans: Vec<Range<usize>>,
    end: usize,
    next: usize,
    in_fence: bool,
}

struct Finder<'s> {
    source: &'s str,
    file: Option<String>,
    index: LineIndex,
    stack: Vec<Open>,
    found: Vec<Occurrence>,
}

/// The occurrences in tokens a tokenizer made from `source`, with the parser's arguments
/// (`file`, `strip_bom`): the parser's walk over them, for a caller that has the tokens.
pub fn from_tokens(tokens: &[Token], source: &str, args: &ParserArgs) -> Vec<Occurrence> {
    let mut finder = Finder {
        source,
        file: args.file.clone(),
        index: LineIndex::new(source),
        stack: vec![Open {
            type_name: "document".into(),
            tag: None,
            occurrence: None,
        }],
        found: Vec::new(),
    };
    let mut positions = Positions::new(source);
    for i in 0..tokens.len() {
        let (span, children) = positions.token(tokens, i);
        let children = children.map(|spans| spans.iter().map(|s| s.start..s.end).collect());
        let mut open: Vec<Walk> = Vec::new();
        open.extend(finder.visit(&tokens[i], span.start..span.end, children, false));
        while let Some(walk) = open.last_mut() {
            let Some(child) = walk.children.get(walk.next) else {
                open.pop();
                finder.stack.pop();
                continue;
            };
            let span = walk
                .spans
                .get(walk.next)
                .cloned()
                .unwrap_or(walk.end..walk.end);
            walk.next += 1;
            let in_fence = walk.in_fence;
            let grandchildren = child
                .children
                .as_ref()
                .map(|c| vec![span.start..span.start; c.len()]);
            open.extend(finder.visit(child, span, grandchildren, in_fence));
        }
    }
    finder.found
}

/// `nodes.at(-1)`'s type and tag as the parser compares them with a closing token's.
fn same(open: &Open, type_name: &str, tag: Option<&str>) -> bool {
    open.type_name == type_name && open.tag.as_deref() == tag
}

impl Finder<'_> {
    fn location(&self, span: &Range<usize>) -> Span {
        let edge = |offset: usize| {
            let (line, character) = self.index.line_col(self.source, offset);
            Position {
                line,
                character: Some(character),
                offset,
            }
        };
        Span {
            file: self.file.clone(),
            start: edge(span.start),
            end: edge(span.end.max(span.start)),
        }
    }

    /// The parser's `handleToken` as far as its stack goes, and the occurrence the token is;
    /// the children to walk when it has pushed a node for them.
    fn visit<'t>(
        &mut self,
        token: &'t Token,
        span: Range<usize>,
        children: Option<Vec<Range<usize>>>,
        in_fence: bool,
    ) -> Option<Walk<'t>> {
        if token.kind == "frontmatter"
            || token.hidden
            || (token.kind == "text" && token.content.is_empty())
        {
            return None;
        }
        if token.kind == "fence" {
            self.fence_tag(token, &span);
        }
        let occurrence = self.occurrence(token, &span, in_fence);
        if token.kind == "annotation" {
            return None;
        }
        let kind = token
            .kind
            .strip_suffix("_open")
            .or_else(|| token.kind.strip_suffix("_close"))
            .unwrap_or(&token.kind);
        let type_name = match kind {
            "ordered_list" | "bullet_list" => "list",
            "code_inline" => "code",
            "list_item" => "item",
            "variable" => "text",
            other => other,
        };
        let meta_tag = token
            .meta
            .as_ref()
            .and_then(|m| m.get("tag"))
            .and_then(|t| t.as_str());
        // A closing token closes the node on top of the stack when that node has its type and
        // tag; otherwise the parser makes it a node of its own (`missing-opening`).
        if token.nesting < 0
            && let Some(top) = self.stack.last()
            && same(top, type_name, meta_tag)
        {
            if let (Some(open), Some(close)) = (top.occurrence, occurrence) {
                self.found[open].partner = Some(close);
                self.found[close].partner = Some(open);
            }
            self.stack.pop();
            return None;
        }
        let node = || Open {
            type_name: type_name.to_string(),
            tag: meta_tag.filter(|t| !t.is_empty()).map(str::to_string),
            occurrence,
        };
        if token.nesting > 0 {
            self.stack.push(node());
        }
        let tokens = token.children.as_deref()?;
        if type_name == "image" {
            return None;
        }
        self.stack.push(node());
        Some(Walk {
            children: tokens,
            spans: children.unwrap_or_default(),
            end: span.end,
            next: 0,
            in_fence: in_fence || token.kind == "fence",
        })
    }

    /// The tag in the info string of the fence `token` (which spans `span`), if the annotations
    /// plugin read one: found again on the fence's first line, after the fence's marker.
    fn fence_tag(&mut self, token: &Token, span: &Range<usize>) {
        if !token.info.contains("{%") {
            return;
        }
        let kind = match &token.grammar_meta {
            Some(meta) => OccurrenceKind::Annotation {
                attributes: attributes(meta),
            },
            None => {
                let Some(message) = token.errors.iter().flatten().find_map(|e| {
                    let message = e.get("message")?.as_str()?;
                    (e.get("id")?.as_str()? == "fence-tag-error").then_some(message)
                }) else {
                    return;
                };
                OccurrenceKind::Error {
                    message: message
                        .strip_prefix("Syntax error in fence tag: ")
                        .unwrap_or(message)
                        .to_string(),
                }
            }
        };
        let line_end = self.source[span.start..]
            .find(['\n', '\r'])
            .map_or(self.source.len(), |i| span.start + i);
        let line = &self.source[span.start..line_end];
        let marker = token.markup.len().min(line.len());
        let Some(open) = line.get(marker..).and_then(|rest| rest.find("{%")) else {
            return;
        };
        let start = span.start + marker + open;
        // The `%}` the tokenizer's `findTagEnd` finds (quotes respected), else the line's end.
        let units = crate::markdown_it::js::to_utf16(&self.source[start..line_end]);
        let end = match crate::next::tokenizer::utils::find_tag_end(&units, 0) {
            Some(close) => start + String::from_utf16_lossy(&units[..close + 2]).len(),
            None => line_end,
        };
        let span = start..end.min(line_end);
        self.found.push(Occurrence {
            kind,
            location: self.location(&span),
            span,
            in_fence: false,
            partner: None,
        });
    }

    /// The occurrence `token` is, if it is a tag; its index.
    fn occurrence(&mut self, token: &Token, span: &Range<usize>, in_fence: bool) -> Option<usize> {
        let meta = token.grammar_meta.as_deref();
        let name = || meta.and_then(|m| m.tag.clone()).unwrap_or_default();
        let attributes = || meta.map(attributes).unwrap_or_default();
        let kind = match token.kind.as_str() {
            "tag_open" => OccurrenceKind::Open {
                name: name(),
                attributes: attributes(),
            },
            "tag" => OccurrenceKind::SelfClosing {
                name: name(),
                attributes: attributes(),
            },
            "tag_close" => OccurrenceKind::Close { name: name() },
            "annotation" => OccurrenceKind::Annotation {
                attributes: attributes(),
            },
            "variable" => OccurrenceKind::Interpolation {
                value: meta
                    .and_then(|m| m.variable.as_ref())
                    .map_or(Scalar::Undefined, scalar),
            },
            "error" => OccurrenceKind::Error {
                message: token
                    .meta
                    .as_ref()
                    .and_then(|m| m.get("error"))
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or_default()
                    .to_string(),
            },
            _ => return None,
        };
        // A tag spans its `{% %}`; a span the positions could not place is left as found.
        let span = span.clone();
        self.found.push(Occurrence {
            kind,
            location: self.location(&span),
            span,
            in_fence,
            partner: None,
        });
        Some(self.found.len() - 1)
    }
}

/// A tag's attributes as the grammar gives them: each `name=value` (and the primary value,
/// `#id`) in order, `.name` collected into a `class` object. (Where the parser would throw,
/// `.name` after a `class` that is not an object, the `.name` is left out.)
fn attributes(meta: &grammar::Meta) -> IndexMap<String, Scalar> {
    let mut out = IndexMap::new();
    for attribute in meta.attributes.iter().flatten().flatten() {
        match attribute.kind {
            AttributeType::Attribute => {
                js_insert(&mut out, attribute.name.clone(), scalar(&attribute.value));
            }
            AttributeType::Class => match out.get_mut("class") {
                Some(Scalar::Object(class)) => {
                    js_insert(class, attribute.name.clone(), scalar(&attribute.value));
                }
                Some(_) => {}
                None => {
                    let mut class = IndexMap::new();
                    js_insert(&mut class, attribute.name.clone(), scalar(&attribute.value));
                    js_insert(&mut out, "class".to_string(), Scalar::Object(class));
                }
            },
        }
    }
    out
}

/// A grammar value as the AST holds it (the grammar nests values no deeper than
/// [`grammar::MAX_NESTING`]).
fn scalar(value: &grammar::Value) -> Scalar {
    let object = |o: &grammar::Object| {
        let mut out = IndexMap::new();
        for (k, v) in o.entries() {
            js_insert(&mut out, k.to_string(), scalar(v));
        }
        out
    };
    match value {
        grammar::Value::Null => Scalar::Null,
        grammar::Value::Bool(b) => Scalar::Boolean(*b),
        grammar::Value::Number(n) => Scalar::Number(*n),
        grammar::Value::String(s) => Scalar::String(s.clone()),
        grammar::Value::Array(items) => Scalar::Array(items.iter().map(scalar).collect()),
        grammar::Value::Object(o) => Scalar::Object(object(o)),
        grammar::Value::Variable(v) => Scalar::Variable(ast::Variable::new(
            v.path
                .iter()
                .map(|step| match step {
                    grammar::Value::Number(n) => PathSegment::Number(*n),
                    grammar::Value::String(s) => PathSegment::String(s.clone()),
                    other => PathSegment::String(other.to_json_string()),
                })
                .collect(),
        )),
        grammar::Value::Function(f) => {
            Scalar::Function(ast::Function::new(f.name.clone(), object(&f.parameters)))
        }
    }
}

#[cfg(test)]
mod tests;
