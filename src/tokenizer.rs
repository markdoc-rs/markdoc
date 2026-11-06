use crate::tag_parser::{ParsedTag, TagKind};
use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag as CmarkTag, TagEnd,
};

/// Name a CommonMark column alignment for storage on the table node's
/// `align` attribute. `None` (no explicit `:--`/`--:` marker) maps to an
/// empty string so the renderer treats that column as default.
fn align_name(a: &Alignment) -> String {
    match a {
        Alignment::Left => "left",
        Alignment::Center => "center",
        Alignment::Right => "right",
        Alignment::None => "",
    }
    .to_string()
}

const SENTINEL_OPEN: char = '\u{E000}';
const SENTINEL_CLOSE: char = '\u{E001}';

#[derive(Debug, Clone)]
pub struct Token {
    pub event: TokenEvent,
    pub position: Option<(usize, usize)>,
}

#[derive(Debug, Clone)]
pub enum TokenEvent {
    Start(TokenType),
    End(TokenType),
    Text(String),
    Code(String),
    Html(String),
    /// A Markdoc tag — open, close, self-closing, or heading-id sugar.
    Tag(TagKind),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenType {
    Paragraph,
    Heading(usize),
    BlockQuote,
    CodeBlock(Option<String>),
    List(bool, Option<u64>), // ordered, start
    Item,
    Table(Vec<String>), // per-column alignment names ("left"/"center"/"right"/"")
    TableHead,
    TableRow,
    TableCell,
    Emphasis,
    Strong,
    Strikethrough,
    Link(String, String),  // url, title
    Image(String, String), // url, title
    Rule,
    LineBreak,
    SoftBreak,
    /// CommonMark footnote reference `[^name]` (an inline atom).
    FootnoteRef(String),
    /// CommonMark footnote definition `[^name]: …` (a block wrapping its body).
    FootnoteDef(String),
}

pub struct Tokenizer {
    options: Options,
}

impl Tokenizer {
    pub fn new() -> Self {
        let mut options = Options::empty();
        options.insert(Options::ENABLE_TABLES);
        options.insert(Options::ENABLE_FOOTNOTES);
        options.insert(Options::ENABLE_STRIKETHROUGH);
        options.insert(Options::ENABLE_TASKLISTS);
        options.insert(Options::ENABLE_SMART_PUNCTUATION);

        Self { options }
    }

    pub fn tokenize(&self, content: &str) -> Vec<Token> {
        self.tokenize_with_tags(content, &[])
    }

    /// Tokenize content that may contain Markdoc tag sentinels and the
    /// corresponding parsed-tag table produced by `tag_parser::segment_with_tags`.
    ///
    /// Text events containing sentinels are split: text-before, then the tag
    /// event, then any remainder (recursively).
    pub fn tokenize_with_tags(&self, content: &str, tags: &[ParsedTag]) -> Vec<Token> {
        let parser = Parser::new_ext(content, self.options);
        let mut tokens = Vec::new();

        for (event, range) in parser.into_offset_iter() {
            let position = Some((range.start, range.end));

            match event {
                Event::Start(tag) => {
                    if let Some(token_type) = self.convert_tag(&tag) {
                        tokens.push(Token {
                            event: TokenEvent::Start(token_type),
                            position,
                        });
                    }
                }
                Event::End(tag_end) => {
                    if let Some(token_type) = self.convert_tag_end(&tag_end) {
                        tokens.push(Token {
                            event: TokenEvent::End(token_type),
                            position,
                        });
                    }
                }
                Event::Text(text) => {
                    push_text_with_tags(&mut tokens, &text, tags, position);
                }
                Event::Code(code) => {
                    // Inline code is taken verbatim — no sentinel scan.
                    tokens.push(Token {
                        event: TokenEvent::Code(code.to_string()),
                        position,
                    });
                }
                Event::Html(html) | Event::InlineHtml(html) => {
                    push_text_with_tags_as_html(&mut tokens, &html, tags, position);
                }
                // Self-closing atoms — emit Start + End so the parser
                // stack stays balanced. Without the matching End, these
                // nodes remained on top of the stack and absorbed all
                // subsequent siblings as children.
                Event::SoftBreak => {
                    tokens.push(Token {
                        event: TokenEvent::Start(TokenType::SoftBreak),
                        position,
                    });
                    tokens.push(Token {
                        event: TokenEvent::End(TokenType::SoftBreak),
                        position,
                    });
                }
                Event::HardBreak => {
                    tokens.push(Token {
                        event: TokenEvent::Start(TokenType::LineBreak),
                        position,
                    });
                    tokens.push(Token {
                        event: TokenEvent::End(TokenType::LineBreak),
                        position,
                    });
                }
                Event::Rule => {
                    tokens.push(Token {
                        event: TokenEvent::Start(TokenType::Rule),
                        position,
                    });
                    tokens.push(Token {
                        event: TokenEvent::End(TokenType::Rule),
                        position,
                    });
                }
                // A `[^name]` reference — emit as a self-closing atom (Start
                // + End) so the parse stack stays balanced. The definition it
                // points at arrives as its own `FootnoteDefinition` block;
                // `resolve_footnotes` later matches the two.
                Event::FootnoteReference(name) => {
                    let ft = TokenType::FootnoteRef(name.to_string());
                    tokens.push(Token {
                        event: TokenEvent::Start(ft.clone()),
                        position,
                    });
                    tokens.push(Token {
                        event: TokenEvent::End(ft),
                        position,
                    });
                }
                _ => continue,
            }
        }

        tokens
    }

    fn convert_tag_end(&self, tag_end: &TagEnd) -> Option<TokenType> {
        match tag_end {
            TagEnd::Paragraph => Some(TokenType::Paragraph),
            TagEnd::Heading(_) => {
                // We can't determine level from TagEnd, use generic heading
                Some(TokenType::Heading(1))
            }
            TagEnd::BlockQuote => Some(TokenType::BlockQuote),
            TagEnd::CodeBlock => Some(TokenType::CodeBlock(None)),
            TagEnd::List(_) => Some(TokenType::List(false, None)),
            TagEnd::Item => Some(TokenType::Item),
            TagEnd::Table => Some(TokenType::Table(Vec::new())),
            TagEnd::TableHead => Some(TokenType::TableHead),
            TagEnd::TableRow => Some(TokenType::TableRow),
            TagEnd::TableCell => Some(TokenType::TableCell),
            TagEnd::Emphasis => Some(TokenType::Emphasis),
            TagEnd::Strong => Some(TokenType::Strong),
            TagEnd::Strikethrough => Some(TokenType::Strikethrough),
            TagEnd::Link => Some(TokenType::Link(String::new(), String::new())),
            TagEnd::Image => Some(TokenType::Image(String::new(), String::new())),
            TagEnd::FootnoteDefinition => Some(TokenType::FootnoteDef(String::new())),
            _ => None,
        }
    }

    fn convert_tag(&self, tag: &CmarkTag) -> Option<TokenType> {
        match tag {
            CmarkTag::Paragraph => Some(TokenType::Paragraph),
            CmarkTag::FootnoteDefinition(name) => Some(TokenType::FootnoteDef(name.to_string())),
            CmarkTag::Heading { level, .. } => {
                let h_level = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                Some(TokenType::Heading(h_level))
            }
            CmarkTag::BlockQuote(_) => Some(TokenType::BlockQuote),
            CmarkTag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(lang) => {
                        if lang.is_empty() {
                            None
                        } else {
                            Some(lang.to_string())
                        }
                    }
                    CodeBlockKind::Indented => None,
                };
                Some(TokenType::CodeBlock(lang))
            }
            CmarkTag::List(start_num) => {
                if let Some(num) = start_num {
                    Some(TokenType::List(true, Some(*num)))
                } else {
                    Some(TokenType::List(false, None))
                }
            }
            CmarkTag::Item => Some(TokenType::Item),
            CmarkTag::Table(aligns) => {
                Some(TokenType::Table(aligns.iter().map(align_name).collect()))
            }
            CmarkTag::TableHead => Some(TokenType::TableHead),
            CmarkTag::TableRow => Some(TokenType::TableRow),
            CmarkTag::TableCell => Some(TokenType::TableCell),
            CmarkTag::Emphasis => Some(TokenType::Emphasis),
            CmarkTag::Strong => Some(TokenType::Strong),
            CmarkTag::Strikethrough => Some(TokenType::Strikethrough),
            CmarkTag::Link {
                dest_url, title, ..
            } => Some(TokenType::Link(dest_url.to_string(), title.to_string())),
            CmarkTag::Image {
                dest_url, title, ..
            } => Some(TokenType::Image(dest_url.to_string(), title.to_string())),
            _ => None,
        }
    }
}

impl Default for Tokenizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Scan `text` for tag sentinels and split it into a sequence of
/// `Text` and `Tag` token events appended to `tokens`.
fn push_text_with_tags(
    tokens: &mut Vec<Token>,
    text: &str,
    tags: &[ParsedTag],
    position: Option<(usize, usize)>,
) {
    if !text.contains(SENTINEL_OPEN) || tags.is_empty() {
        if !text.is_empty() {
            tokens.push(Token {
                event: TokenEvent::Text(text.to_string()),
                position,
            });
        }
        return;
    }
    for piece in scan_sentinels(text, tags) {
        match piece {
            SentinelPiece::Text(s) if !s.is_empty() => tokens.push(Token {
                event: TokenEvent::Text(s),
                position,
            }),
            SentinelPiece::Tag(kind, span) => tokens.push(Token {
                event: TokenEvent::Tag(kind),
                position: Some((span.start, span.end)),
            }),
            SentinelPiece::Text(_) => {} // empty
        }
    }
}

/// Same as `push_text_with_tags` but for cmark `Html` events. Tag
/// sentinels are still extracted; intervening HTML chunks are emitted
/// as `TokenEvent::Html` so the parser can decide what to do with them
/// (currently dropped, matching prior behaviour).
fn push_text_with_tags_as_html(
    tokens: &mut Vec<Token>,
    html: &str,
    tags: &[ParsedTag],
    position: Option<(usize, usize)>,
) {
    if !html.contains(SENTINEL_OPEN) || tags.is_empty() {
        if !html.is_empty() {
            tokens.push(Token {
                event: TokenEvent::Html(html.to_string()),
                position,
            });
        }
        return;
    }
    for piece in scan_sentinels(html, tags) {
        match piece {
            SentinelPiece::Text(s) if !s.is_empty() => tokens.push(Token {
                event: TokenEvent::Html(s),
                position,
            }),
            SentinelPiece::Tag(kind, span) => tokens.push(Token {
                event: TokenEvent::Tag(kind),
                position: Some((span.start, span.end)),
            }),
            SentinelPiece::Text(_) => {}
        }
    }
}

enum SentinelPiece {
    Text(String),
    /// A tag, and **its own** span in the source -- not the span of the
    /// cmark event it was found inside. A paragraph is one Text event; the
    /// three tags in it are three different places, and a diagnostic that
    /// points at the paragraph points at none of them.
    Tag(TagKind, std::ops::Range<usize>),
}

fn scan_sentinels(text: &str, tags: &[ParsedTag]) -> Vec<SentinelPiece> {
    let mut out = Vec::new();
    let mut last = 0usize;
    let mut cursor = 0usize;
    while let Some(rel) = text[cursor..].find(SENTINEL_OPEN) {
        let abs_open = cursor + rel;
        if abs_open > last {
            out.push(SentinelPiece::Text(text[last..abs_open].to_string()));
        }
        let after_open = abs_open + SENTINEL_OPEN.len_utf8();
        match text[after_open..].find(SENTINEL_CLOSE) {
            Some(close_rel) => {
                let abs_close = after_open + close_rel;
                let idx_str = &text[after_open..abs_close];
                let next = abs_close + SENTINEL_CLOSE.len_utf8();
                if let Ok(idx) = idx_str.parse::<usize>()
                    && let Some(tag) = tags.get(idx)
                {
                    out.push(SentinelPiece::Tag(
                        tag.kind.clone(),
                        tag.source_range.clone(),
                    ));
                }
                // If idx is out of range, the sentinel is silently dropped.
                last = next;
                cursor = next;
            }
            None => {
                // No closing sentinel — bail; preserve the rest as-is.
                break;
            }
        }
    }
    if last < text.len() {
        out.push(SentinelPiece::Text(text[last..].to_string()));
    }
    out
}
