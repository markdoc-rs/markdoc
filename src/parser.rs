use crate::ast::Node;
use crate::frontmatter;
use crate::tag_parser::{self, AttrValue, TagKind};
use crate::tokenizer::{Token, TokenEvent, TokenType, Tokenizer};
use crate::types::*;
use std::collections::HashMap;

/// Private-use sentinel standing in for an escaped `\{%` while the tag
/// scanners run, so they don't treat it as a tag opener. Restored to a
/// literal `{%` in node content by [`unescape_tag_literals`] afterwards.
const ESCAPED_TAG_OPEN: &str = "\u{E010}";

/// Turns a byte range in the **original** file into a `Location`.
///
/// Holds the source and an index over it, because the two conversions a
/// caller needs -- byte offset to line, byte offset to UTF-16 column -- both
/// want the whole file and neither is cheap to redo per node. One of these is
/// built per `parse()` and borrowed by everything that makes a node.
struct Spans<'a> {
    src: &'a str,
    index: crate::sourcemap::LineIndex,
    file: Option<String>,
}

impl<'a> Spans<'a> {
    fn new(src: &'a str, file: Option<String>) -> Self {
        Self {
            src,
            index: crate::sourcemap::LineIndex::new(src),
            file,
        }
    }

    fn edge(&self, offset: usize) -> LocationEdge {
        // Snap to a character boundary. A map that lands in the middle of a
        // multi-byte character is not a position anybody can use: slicing
        // there panics, and an editor counting UTF-16 units cannot be given
        // half of one. It happens inside a `{% table %}` block, whose
        // interior is parsed from an extracted copy and whose nodes are
        // therefore located to the block rather than to the cell -- see
        // `list_table`. Rounding down keeps the offset inside the construct
        // it belongs to.
        let offset = offset.min(self.src.len());
        let offset = (0..=offset)
            .rev()
            .find(|&o| self.src.is_char_boundary(o))
            .unwrap_or(0);
        let (line, character) = self.index.line_col(self.src, offset);
        LocationEdge {
            line,
            character: Some(character),
            offset,
        }
    }

    /// `None` for a node whose token carried no position -- which is not a
    /// failure, only an absence: the list-table pre-pass and the frontmatter
    /// strip both produce nodes that were never a span of the file.
    fn location(&self, span: Option<(usize, usize)>) -> Option<Location> {
        let (start, end) = span?;
        Some(Location {
            file: self.file.clone(),
            start: self.edge(start),
            end: self.edge(end.max(start)),
        })
    }

    /// The line a span begins on, for `Node.lines`.
    fn line_of(&self, span: Option<(usize, usize)>) -> usize {
        span.map(|(s, _)| self.index.line_col(self.src, s).0)
            .unwrap_or(1)
    }
}

pub fn parse(content: &str, args: Option<ParserArgs>) -> Result<Node> {
    // Every rewrite below moves bytes, and each records what it moved so a
    // position in the final buffer can be traced back to the file the author
    // typed. Without that chain an offset here indexes a string nobody has
    // ever seen, which is why positions used to be discarded entirely.
    let mut maps = crate::sourcemap::SourceMapChain::new();

    // The file as the author has it open, kept for the whole of this
    // function. Every rewrite below shadows `content`; positions are mapped
    // back to THIS, so this is the string they index and this is what line
    // and column are counted against.
    let original = content;

    // A byte-order mark, if the file has one.
    //
    // pulldown-cmark does not strip it, so `\u{FEFF}# Title` is not a
    // heading -- it is a paragraph that happens to start with a `#`. Two
    // files in the corpus are like that, and the website generator already
    // strips a BOM before calling this for exactly that reason. Doing it
    // here instead means every caller is covered, including the ones that
    // read a file straight off disk and have no such workaround.
    //
    // Three bytes removed, recorded like any other rewrite, so positions
    // still point into the file the author has open.
    let (content, bom_map) = {
        let mut m = crate::sourcemap::SourceMap::identity();
        match content.strip_prefix('\u{FEFF}') {
            Some(rest) => {
                m.replace(0..0, 0..'\u{FEFF}'.len_utf8());
                (rest, m)
            }
            None => (content, m),
        }
    };
    maps.push(bom_map);

    // Extract frontmatter.
    let (frontmatter_data, content_without_fm, fm_offset) =
        frontmatter::extract_frontmatter_mapped(content)?;
    let mut fm_map = crate::sourcemap::SourceMap::identity();
    if fm_offset > 0 {
        fm_map.replace(0..0, 0..fm_offset);
    }
    maps.push(fm_map);

    // Neutralise escaped tag openers (`\{%`) before any tag scanner runs, so
    // they are never treated as tags. The sentinel is restored to a literal
    // `{%` in node content after parsing (`unescape_tag_literals`).
    let (content_without_fm, escape_map) =
        crate::sourcemap::replace_mapped(&content_without_fm, "\\{%", ESCAPED_TAG_OPEN);
    maps.push(escape_map);

    // Inline `{% $var %}` / `{% func(...) %}` interpolation is parsed into
    // interpolation nodes (by `segment_with_tags`) and resolved against the
    // evaluation `Context` at transform time — the same path as tag-attribute
    // expressions — so no text-level substitution happens here.

    // 1. Pre-extract Markdoc list-syntax `{% table %}` blocks. Their inner
    //    `---`/`*` syntax must be parsed here, before pulldown-cmark mangles
    //    it; each block becomes a placeholder spliced back in after parsing.
    let (content_with_tables, list_tables, table_map) =
        crate::list_table::extract_list_tables_mapped(&content_without_fm);
    maps.push(table_map);

    // 2. Extract structural tags (`{% name attrs %}`, `{% /name %}`, etc.)
    //    into a side-table; replace each occurrence with a sentinel so the
    //    markdown tokenizer leaves them as opaque text. The tokenizer
    //    re-merges tag events back into the token stream.
    let (content_with_sentinels, mut parsed_tags, sentinel_map) =
        tag_parser::segment_with_tags_mapped(&content_with_tables);

    // Two coordinate systems meet here, and conflating them is how markdown
    // positions came to be wrong after the first tag in a file.
    //
    // A tag's own span was measured in `content_with_tables`, BEFORE it was
    // replaced by a sentinel -- the tag does not exist in the buffer the
    // tokenizer reads. So it is translated now, through the three maps built
    // so far.
    for tag in &mut parsed_tags {
        tag.source_range = maps.map_range(tag.source_range.clone());
    }

    let tokenizer = Tokenizer::new();
    let mut tokens = tokenizer.tokenize_with_tags(&content_with_sentinels, &parsed_tags);

    // Everything cmark reported, on the other hand, is measured in
    // `content_with_sentinels`, so it needs the fourth map as well. A
    // sentinel is three to five bytes where the tag may have been eighty.
    maps.push(sentinel_map);
    for token in &mut tokens {
        // A tag token's position is the tag's own span, already translated
        // above. Running it through the chain again would map a position
        // that is no longer in the chain's input space.
        if matches!(token.event, TokenEvent::Tag(_)) {
            continue;
        }
        if let Some((start, end)) = token.position {
            let mapped = maps.map_range(start..end);
            token.position = Some((mapped.start, mapped.end));
        }
    }

    let tokens = lift_block_tags(tokens);
    let spans = Spans::new(original, args.as_ref().and_then(|a| a.file.clone()));
    let mut doc = parse_tokens(tokens, args, &spans)?;

    // Swap the list-table placeholders for their parsed `Table` nodes.
    crate::list_table::splice_list_tables(&mut doc, &list_tables);

    // Restore escaped `\{%` (now the sentinel) to a literal `{%` in text and
    // code content, after every tag scanner has run.
    unescape_tag_literals(&mut doc);

    if let Some(fm) = frontmatter_data {
        doc.attributes
            .insert("frontmatter".to_string(), Scalar::Object(fm));
    }

    Ok(doc)
}

/// Restore escaped tag openers ([`ESCAPED_TAG_OPEN`]) to a literal `{%` in
/// every node's text `content`, after the tag scanners have run.
fn unescape_tag_literals(node: &mut Node) {
    if let Some(Scalar::String(s)) = node.attributes.get_mut("content")
        && s.contains(ESCAPED_TAG_OPEN)
    {
        *s = s.replace(ESCAPED_TAG_OPEN, "{%");
    }
    for child in &mut node.children {
        unescape_tag_literals(child);
    }
}

/// Lift block-level Markdoc tags out of the paragraphs pulldown-cmark
/// wraps them in.
///
/// Tags are replaced with inline sentinels before markdown tokenization,
/// so pulldown sees a lone-on-its-line `{% tag %}` as inline text and
/// wraps it in a paragraph. A blank line inside a block tag then splits
/// the content into two paragraphs — the open sentinel trapped in the
/// first, the close sentinel in the second — and the first paragraph's
/// `End` event pops the still-open tag off the parse stack, truncating
/// everything after the blank line (it escapes as a sibling).
///
/// This pass walks each paragraph and splits it at every block-level tag
/// (a Markdoc `Open` / `Close` / `SelfClose` that sits alone on its
/// line — i.e. is bounded by a soft break or the paragraph edge on both
/// sides). Each such tag is emitted at block level; the inline runs
/// between them are re-wrapped as their own paragraphs. The result is
/// that `{% callout %}` spanning blank lines becomes a real container,
/// and `{% else /%}` stays a direct child of `{% if %}`. Tags that share
/// a line with other text (`see {% tagref /%} here`) are left inline.
fn lift_block_tags(tokens: Vec<Token>) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        if matches!(tokens[i].event, TokenEvent::Start(TokenType::Paragraph)) {
            // Find the matching End(Paragraph). Paragraphs are leaf
            // blocks in CommonMark, so the next End(Paragraph) is ours.
            let para_start = &tokens[i];
            let end = (i + 1..tokens.len())
                .find(|&j| matches!(tokens[j].event, TokenEvent::End(TokenType::Paragraph)));
            let Some(end) = end else {
                // Unbalanced (shouldn't happen) — copy verbatim.
                out.push(tokens[i].clone());
                i += 1;
                continue;
            };
            let inner = &tokens[i + 1..end];
            let para_end = &tokens[end];
            split_paragraph_inner(inner, para_start, para_end, &mut out);
            i = end + 1;
        } else {
            out.push(tokens[i].clone());
            i += 1;
        }
    }
    out
}

/// Split one paragraph's `inner` token run at each block-level tag,
/// appending the result to `out`. `para_start` / `para_end` supply the
/// `Start`/`End(Paragraph)` tokens reused to re-wrap each inline segment.
fn split_paragraph_inner(
    inner: &[Token],
    para_start: &Token,
    para_end: &Token,
    out: &mut Vec<Token>,
) {
    let n = inner.len();
    // Mark which indices are block-level tags (alone on their line).
    let is_block: Vec<bool> = (0..n).map(|k| is_block_tag_at(inner, k)).collect();
    if !is_block.iter().any(|b| *b) {
        // No block tags — emit the paragraph untouched.
        out.push(para_start.clone());
        out.extend_from_slice(inner);
        out.push(para_end.clone());
        return;
    }

    let mut seg: Vec<Token> = Vec::new();
    let mut k = 0;
    while k < n {
        if is_block[k] {
            // Drop a soft break that trails the current segment (the one
            // that separated it from this tag) before flushing.
            trim_trailing_softbreak(&mut seg);
            flush_segment(&mut seg, para_start, para_end, out);
            out.push(inner[k].clone()); // the tag, now block-level
            // Skip a soft break immediately following the tag.
            if k + 2 < n && is_sb_start(&inner[k + 1].event) && is_sb_end(&inner[k + 2].event) {
                k += 3;
            } else {
                k += 1;
            }
        } else {
            seg.push(inner[k].clone());
            k += 1;
        }
    }
    trim_trailing_softbreak(&mut seg);
    flush_segment(&mut seg, para_start, para_end, out);
}

/// True when `inner[k]` is a Markdoc block container tag (`Open` /
/// `Close` / `SelfClose`) that is alone on its line — bounded on each
/// side by a soft break or the paragraph edge. Heading-id sugar and
/// inline tags (sharing a line with text) return false.
fn is_block_tag_at(inner: &[Token], k: usize) -> bool {
    let is_tag = matches!(
        inner[k].event,
        TokenEvent::Tag(TagKind::Open { .. } | TagKind::Close { .. } | TagKind::SelfClose { .. })
    );
    if !is_tag {
        return false;
    }
    let left_ok =
        k == 0 || (k >= 2 && is_sb_end(&inner[k - 1].event) && is_sb_start(&inner[k - 2].event));
    let right_ok = k + 1 == inner.len()
        || (k + 2 < inner.len()
            && is_sb_start(&inner[k + 1].event)
            && is_sb_end(&inner[k + 2].event));
    left_ok && right_ok
}

fn is_sb_start(ev: &TokenEvent) -> bool {
    matches!(ev, TokenEvent::Start(TokenType::SoftBreak))
}

fn is_sb_end(ev: &TokenEvent) -> bool {
    matches!(ev, TokenEvent::End(TokenType::SoftBreak))
}

/// Remove a trailing soft break (Start+End pair) from a segment.
fn trim_trailing_softbreak(seg: &mut Vec<Token>) {
    let n = seg.len();
    if n >= 2 && is_sb_start(&seg[n - 2].event) && is_sb_end(&seg[n - 1].event) {
        seg.truncate(n - 2);
    }
}

/// Wrap a non-empty inline segment as a paragraph and append it to
/// `out`, draining `seg`. A leading soft break is trimmed first; an
/// all-whitespace / empty segment emits nothing.
fn flush_segment(seg: &mut Vec<Token>, para_start: &Token, para_end: &Token, out: &mut Vec<Token>) {
    // Trim a leading soft break.
    if seg.len() >= 2 && is_sb_start(&seg[0].event) && is_sb_end(&seg[1].event) {
        seg.drain(0..2);
    }
    if seg.is_empty() {
        return;
    }
    out.push(para_start.clone());
    out.append(seg);
    out.push(para_end.clone());
}

fn parse_tokens(tokens: Vec<Token>, args: Option<ParserArgs>, spans: &Spans) -> Result<Node> {
    let args = args.unwrap_or(ParserArgs {
        file: None,
        slots: false,
        location: true,
    });

    let mut doc = Node::new(NodeType::Document, HashMap::new(), Vec::new(), None);
    let mut stack: Vec<Node> = vec![doc.clone()];
    // Syntax errors found while building the tree. They are collected rather
    // than returned, because `parse` is used by a renderer and by a linter
    // and the two want opposite things from a bad document: the renderer
    // wants the rest of it, the linter wants every fault at once. Returning
    // on the first would give neither.
    let mut errors: Vec<ValidationError> = Vec::new();

    for token in tokens {
        let span = token.position;
        match token.event {
            TokenEvent::Start(token_type) => {
                let node = create_node_from_token(&token_type, span, spans, &args);
                stack.push(node);
            }
            TokenEvent::End(_token_type) => {
                if stack.len() > 1 {
                    // `close_to_tag` may have already attached open markdown
                    // containers (lists, paragraphs, …) to a Markdoc tag that
                    // closed mid-structure. Pulldown-cmark still emits the
                    // matching End events afterward; ignoring them when a
                    // Markdoc tag is on top keeps `{% if %}` / `{% callout %}`
                    // from being popped early (which would leak later siblings
                    // out of the conditional — e.g. a list inside a callout
                    // inside an if, then a blank line, then another callout).
                    if matches!(stack.last().map(|n| &n.node_type), Some(NodeType::Tag)) {
                        continue;
                    }
                    let node = stack.pop().unwrap();

                    if let Some(parent) = stack.last_mut() {
                        if args.slots && is_slot_node(&node) {
                            if let Some(Scalar::String(slot_name)) = node.attributes.get("primary")
                            {
                                parent.slots.insert(slot_name.clone(), node);
                            } else {
                                parent.push(node);
                            }
                        } else {
                            parent.push(node);
                        }
                    }
                }
            }
            TokenEvent::Text(text) => {
                if let Some(parent) = stack.last_mut() {
                    let mut attrs = HashMap::new();
                    attrs.insert("content".to_string(), Scalar::String(text));
                    let mut text_node = Node::new(NodeType::Text, attrs, Vec::new(), None);
                    // A leaf, and the only kind of node that holds prose. A
                    // style checker reporting "passive voice" with nothing to
                    // point at is a list of complaints about a file.
                    locate(&mut text_node, span, spans, &args);
                    parent.push(text_node);
                }
            }
            TokenEvent::Code(code) => {
                if let Some(parent) = stack.last_mut() {
                    let mut attrs = HashMap::new();
                    attrs.insert("content".to_string(), Scalar::String(code));
                    let mut code_node = Node::new(NodeType::Code, attrs, Vec::new(), None);
                    locate(&mut code_node, span, spans, &args);
                    parent.push(code_node);
                }
            }
            TokenEvent::Html(html) => {
                // Markdoc has no raw-HTML support. Rather than silently
                // dropping the markup — which loses literal placeholder
                // text such as `<insert …>` — surface it as text so it
                // renders verbatim. HTML comments stay hidden.
                if !html.trim_start().starts_with("<!--")
                    && let Some(parent) = stack.last_mut()
                {
                    let mut attrs = HashMap::new();
                    attrs.insert("content".to_string(), Scalar::String(html));
                    let mut text_node = Node::new(NodeType::Text, attrs, Vec::new(), None);
                    locate(&mut text_node, span, spans, &args);
                    parent.push(text_node);
                }
            }
            TokenEvent::Tag(kind) => {
                handle_tag_event(kind, &mut stack, span, spans, &args, &mut errors);
            }
        }
    }

    // What is still on the stack is what nobody closed.
    //
    // Markdown containers can legitimately be here: `TokenEvent::End` is
    // skipped while a Markdoc tag is on top of the stack (see the note
    // there), so a paragraph inside an unclosed `{% if %}` never got its
    // End. A **tag** that is still open is different -- it is a document
    // the author did not finish, and until now it silently swallowed
    // everything after it to the end of the file.
    while stack.len() > 1 {
        let node = stack.pop().unwrap();
        if let (NodeType::Tag, Some(name)) = (&node.node_type, node.tag.as_deref())
            && name != INTERPOLATION_TAG
        {
            errors.push(ValidationError {
                id: "unclosed-tag".to_string(),
                level: ValidationLevel::Error,
                message: format!(
                    "`{{% {name} %}}` is never closed. Everything after it to the end of the \
                     file became its content; add `{{% /{name} %}}`."
                ),
                location: node.location.clone(),
            });
        }
        if let Some(parent) = stack.last_mut() {
            parent.push(node);
        }
    }

    doc = stack.pop().unwrap();
    if args.location {
        doc.lines = vec![1];
        doc.location = spans.location(Some((0, spans.src.len())));
    }
    doc.errors.extend(errors);
    Ok(doc)
}

/// Give a node its position, if positions were asked for and there is one.
fn locate(node: &mut Node, span: Option<(usize, usize)>, spans: &Spans, args: &ParserArgs) {
    if !args.location {
        return;
    }
    if let Some(loc) = spans.location(span) {
        node.lines = vec![loc.start.line];
        node.location = Some(loc);
    }
}

fn create_node_from_token(
    token_type: &TokenType,
    span: Option<(usize, usize)>,
    spans: &Spans,
    args: &ParserArgs,
) -> Node {
    let (node_type, attributes) = match token_type {
        TokenType::Paragraph => (NodeType::Paragraph, HashMap::new()),
        TokenType::Heading(level) => {
            let mut attrs = HashMap::new();
            attrs.insert("level".to_string(), Scalar::Number(*level as f64));
            (NodeType::Heading, attrs)
        }
        TokenType::BlockQuote => (NodeType::Blockquote, HashMap::new()),
        TokenType::CodeBlock(lang) => {
            let mut attrs = HashMap::new();
            if let Some(language) = lang {
                attrs.insert("language".to_string(), Scalar::String(language.clone()));
            }
            (NodeType::Fence, attrs)
        }
        TokenType::List(ordered, start) => {
            let mut attrs = HashMap::new();
            attrs.insert("ordered".to_string(), Scalar::Boolean(*ordered));
            if let Some(start_num) = start {
                attrs.insert("start".to_string(), Scalar::Number(*start_num as f64));
            }
            (NodeType::List, attrs)
        }
        TokenType::Item => (NodeType::Item, HashMap::new()),
        TokenType::Table(aligns) => {
            let mut attrs = HashMap::new();
            // Carry CommonMark per-column alignment (from the `:--`/`--:`
            // delimiter row) so the renderer can align cells. Omitted when
            // no column declares an explicit alignment.
            if aligns.iter().any(|a| !a.is_empty()) {
                attrs.insert(
                    "align".to_string(),
                    Scalar::Array(aligns.iter().cloned().map(Scalar::String).collect()),
                );
            }
            (NodeType::Table, attrs)
        }
        TokenType::TableHead => (NodeType::Thead, HashMap::new()),
        TokenType::TableRow => (NodeType::Tr, HashMap::new()),
        TokenType::TableCell => (NodeType::Td, HashMap::new()),
        TokenType::Emphasis => (NodeType::Em, HashMap::new()),
        TokenType::Strong => (NodeType::Strong, HashMap::new()),
        TokenType::Strikethrough => (NodeType::Strikethrough, HashMap::new()),
        TokenType::Link(url, title) => {
            let mut attrs = HashMap::new();
            attrs.insert("href".to_string(), Scalar::String(url.clone()));
            if !title.is_empty() {
                attrs.insert("title".to_string(), Scalar::String(title.clone()));
            }
            (NodeType::Link, attrs)
        }
        TokenType::Image(url, title) => {
            let mut attrs = HashMap::new();
            attrs.insert("src".to_string(), Scalar::String(url.clone()));
            if !title.is_empty() {
                attrs.insert("title".to_string(), Scalar::String(title.clone()));
            }
            (NodeType::Image, attrs)
        }
        TokenType::Rule => (NodeType::Hr, HashMap::new()),
        TokenType::LineBreak => (NodeType::Hardbreak, HashMap::new()),
        TokenType::SoftBreak => (NodeType::Softbreak, HashMap::new()),
        // CommonMark footnotes — preserved as tag nodes carrying the label;
        // `resolve_footnotes` matches references to definitions later.
        TokenType::FootnoteRef(name) | TokenType::FootnoteDef(name) => {
            let mut attrs = HashMap::new();
            attrs.insert("name".to_string(), Scalar::String(name.clone()));
            (NodeType::Tag, attrs)
        }
    };

    let mut node = Node::new(node_type, attributes, Vec::new(), None);
    match token_type {
        TokenType::FootnoteRef(_) => node.tag = Some("footnote-ref".to_string()),
        TokenType::FootnoteDef(_) => node.tag = Some("footnote-def".to_string()),
        _ => {}
    }

    if args.location {
        node.lines = vec![spans.line_of(span)];
        node.location = spans.location(span);
    }

    node
}

fn is_slot_node(node: &Node) -> bool {
    node.tag.as_deref() == Some("slot")
}

/// Convert a parsed tag event into stack manipulation:
///   - Open       → push a new `Tag` node
///   - Close      → pop nodes until the matching `Tag` node is closed and
///     attached to its parent (closing markdown nodes that were
///     still open are attached too, in source order)
///   - SelfClose  → push then immediately pop
///   - HeadingId  → store the id on the nearest enclosing heading
fn handle_tag_event(
    kind: TagKind,
    stack: &mut Vec<Node>,
    span: Option<(usize, usize)>,
    spans: &Spans,
    args: &ParserArgs,
    errors: &mut Vec<ValidationError>,
) {
    match kind {
        TagKind::Open { name, attrs } => {
            stack.push(make_tag_node(&name, attrs, span, spans, args));
        }
        TagKind::SelfClose { name, attrs } => {
            stack.push(make_tag_node(&name, attrs, span, spans, args));
            close_to_tag(&name, stack);
        }
        TagKind::Close { name } => {
            if !close_to_tag(&name, stack) {
                // A close with nothing open to close. It used to be dropped
                // without a word, which is the worst of both: the document
                // renders, and the thing the author got wrong is invisible.
                errors.push(ValidationError {
                    id: "unmatched-closing-tag".to_string(),
                    level: ValidationLevel::Error,
                    message: format!(
                        "`{{% /{name} %}}` closes a tag that is not open here. Either the \
                         opening `{{% {name} %}}` is missing, or something between them is \
                         closed in the wrong order."
                    ),
                    location: spans.location(span),
                });
            }
        }
        TagKind::HeadingId { id } => {
            // Walk the stack backwards looking for the nearest open heading.
            for node in stack.iter_mut().rev() {
                if matches!(node.node_type, NodeType::Heading) {
                    node.attributes.insert("id".to_string(), Scalar::String(id));
                    return;
                }
            }
            // No enclosing heading — silently drop. (The Markdoc spec
            // restricts this sugar to heading contexts.)
        }
        TagKind::Interpolation { expr } => {
            // An inline leaf carrying the expression under `primary`; the
            // transformer evaluates it against the Context into text.
            let mut node = Node::new(
                NodeType::Tag,
                HashMap::new(),
                Vec::new(),
                Some(INTERPOLATION_TAG.to_string()),
            );
            node.expressions.insert("primary".to_string(), expr);
            node.inline = true;
            // `{% $robot.name %}` is as much a place in the file as any
            // other tag. A linter reporting an unknown variable has nowhere
            // to point without this.
            locate(&mut node, span, spans, args);
            if let Some(parent) = stack.last_mut() {
                parent.push(node);
            }
        }
    }
}

/// Internal tag name for an inline interpolation node (`{% $var %}` /
/// `{% func(...) %}`). Resolved to text by the transformer; never authored.
pub(crate) const INTERPOLATION_TAG: &str = "$interpolation";

/// Build a `Tag` node from parsed attributes, splitting them into the
/// `attributes` map (literal scalars) and the `expressions` map (raw
/// expression source for variables and function calls).
fn make_tag_node(
    name: &str,
    attrs: tag_parser::TagAttrs,
    span: Option<(usize, usize)>,
    spans: &Spans,
    args: &ParserArgs,
) -> Node {
    let mut attributes: HashMap<String, Scalar> = HashMap::new();
    let mut expressions: HashMap<String, String> = HashMap::new();

    if let Some(primary) = attrs.primary {
        match primary {
            AttrValue::Literal(s) => {
                attributes.insert("primary".to_string(), s);
            }
            AttrValue::Expression(src) => {
                expressions.insert("primary".to_string(), src);
            }
        }
    }
    for (key, value) in attrs.named {
        match value {
            AttrValue::Literal(s) => {
                attributes.insert(key, s);
            }
            AttrValue::Expression(src) => {
                expressions.insert(key, src);
            }
        }
    }

    let mut node = Node::new(
        NodeType::Tag,
        attributes,
        Vec::new(),
        Some(name.to_string()),
    );
    node.expressions = expressions;
    if args.location {
        node.lines = vec![spans.line_of(span)];
        node.location = spans.location(span);
    }
    node
}

/// Pop nodes off the stack and attach them to their parents until a `Tag`
/// node with the given name is itself popped and attached. If no such tag
/// is open, the call is a no-op (mismatched close — silently ignored for now;
/// future work: emit a ValidationError).
/// Close the nearest open tag called `name`, attaching everything above it.
///
/// Returns whether there was one. A `false` is a syntax error the caller
/// reports; this function does not know where in the file the close was.
fn close_to_tag(name: &str, stack: &mut Vec<Node>) -> bool {
    // Find the position of the matching tag on the stack.
    let target = stack
        .iter()
        .rposition(|n| matches!(n.node_type, NodeType::Tag) && n.tag.as_deref() == Some(name));
    let Some(target) = target else { return false };

    // Pop everything above the target into their parents.
    while stack.len() > target + 1 {
        let node = stack.pop().unwrap();
        if let Some(parent) = stack.last_mut() {
            parent.push(node);
        }
    }
    // Pop the target itself and attach to its parent.
    let tag_node = stack.pop().unwrap();
    if let Some(parent) = stack.last_mut() {
        parent.push(tag_node);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- positions -------------------------------------------------------

    /// Every node's span, as the text it actually covers in the original
    /// file. Written this way because an offset pair proves nothing on its
    /// own: a test asserting `42..49` passes just as happily when the file
    /// has shifted under it.
    fn spanned(src: &str) -> Vec<(NodeType, Option<String>, String, usize)> {
        fn walk(n: &Node, src: &str, out: &mut Vec<(NodeType, Option<String>, String, usize)>) {
            if let Some(l) = &n.location {
                out.push((
                    n.node_type.clone(),
                    n.tag.clone(),
                    src.get(l.start.offset..l.end.offset)
                        .unwrap_or("<out of bounds>")
                        .to_string(),
                    l.start.line,
                ));
            }
            for c in &n.children {
                walk(c, src, out);
            }
        }
        let doc = parse(src, None).unwrap();
        let mut out = Vec::new();
        walk(&doc, src, &mut out);
        out
    }

    fn text_spans(src: &str) -> Vec<(String, usize)> {
        spanned(src)
            .into_iter()
            .filter(|(t, _, _, _)| matches!(t, NodeType::Text))
            .map(|(_, _, s, line)| (s, line))
            .collect()
    }

    #[test]
    fn a_nodes_span_is_the_text_it_covers() {
        let src = "# Heading\n\nSome prose.\n";
        assert_eq!(
            text_spans(src),
            vec![("Heading".to_string(), 1), ("Some prose.".to_string(), 3)]
        );
    }

    #[test]
    fn frontmatter_does_not_shift_the_lines_below_it() {
        // The first of the four rewrites. Without its map every position in
        // every document with frontmatter -- which is nearly all of them --
        // is short by the length of the frontmatter.
        let src = "---\ntitle: x\nfoo: bar\n---\n\n# Heading\n";
        assert_eq!(text_spans(src), vec![("Heading".to_string(), 6)]);
    }

    #[test]
    fn a_tag_knows_where_it_is_and_not_where_its_paragraph_is() {
        // Three tags on one line are one cmark Text event. Giving each the
        // event's span points all three diagnostics at the same place.
        let src = "{% tag id=\"a\" /%} and {% tag id=\"b\" /%}\n";
        let tags: Vec<String> = spanned(src)
            .into_iter()
            .filter(|(t, _, _, _)| matches!(t, NodeType::Tag))
            .map(|(_, _, s, _)| s)
            .collect();
        assert_eq!(
            tags,
            vec![
                "{% tag id=\"a\" /%}".to_string(),
                "{% tag id=\"b\" /%}".to_string()
            ]
        );
    }

    #[test]
    fn prose_after_a_tag_is_not_shifted_by_the_sentinel() {
        // The fourth rewrite, and the one that had no map: a sentinel is a
        // handful of bytes where the tag was eighty, so every markdown
        // position after the first tag in a file pointed into a buffer
        // nothing else has ever seen. Nothing read those positions, so
        // nothing looked wrong.
        let src = "{% callout type=\"note\" %}\nInside the callout.\n{% /callout %}\n\nAfter.\n";
        assert_eq!(
            text_spans(src),
            vec![
                ("Inside the callout.".to_string(), 2),
                ("After.".to_string(), 5)
            ]
        );
    }

    #[test]
    fn an_escaped_tag_opener_does_not_shift_what_follows() {
        // The second rewrite: `\\{%` becomes a one-char sentinel, losing two
        // bytes, and everything after it moves.
        let src = "Literal \\{% not a tag %} here.\n\nAfter.\n";
        let spans = text_spans(src);
        assert_eq!(spans.last(), Some(&("After.".to_string(), 3)));
    }

    #[test]
    fn a_character_column_counts_utf16_units() {
        // 'é' is two bytes and one UTF-16 unit. An editor counts the latter.
        let src = "café **bold**\n";
        let doc = parse(src, None).unwrap();
        let bold = find(&doc, &|n| matches!(n.node_type, NodeType::Strong)).unwrap();
        let loc = bold.location.as_ref().unwrap();
        assert_eq!(loc.start.line, 1);
        assert_eq!(loc.start.character, Some(5), "five characters precede it");
        assert_eq!(loc.start.offset, 6, "six bytes precede it");
    }

    #[test]
    fn a_byte_order_mark_does_not_eat_the_first_heading() {
        // pulldown-cmark does not strip it, so `\u{FEFF}# Title` was a
        // paragraph that happened to start with a `#`. Two files in the
        // corpus are like that, and the website generator carries its own
        // workaround for exactly this.
        let src = "\u{FEFF}# Heading\n\nText.\n";
        let doc = parse(src, None).unwrap();
        let h = find(&doc, &|n| matches!(n.node_type, NodeType::Heading))
            .expect("the first line is a heading, BOM or no BOM");
        let loc = h.location.as_ref().unwrap();
        // And the position still points into the file as it is on disk,
        // three bytes in -- not into the copy with the BOM removed.
        assert_eq!(&src[loc.start.offset..loc.end.offset], "# Heading\n");
        assert_eq!(loc.start.line, 1);
    }

    #[test]
    fn an_interpolation_knows_where_it_is() {
        let src = "The {% $mir250 %} is heavier.\n";
        let doc = parse(src, None).unwrap();
        let interp = find(&doc, &|n| n.tag.as_deref() == Some(INTERPOLATION_TAG))
            .expect("the variable is a node");
        let loc = interp.location.as_ref().expect("and it has a position");
        assert_eq!(&src[loc.start.offset..loc.end.offset], "{% $mir250 %}");
    }

    #[test]
    fn a_list_table_is_located_but_its_cells_are_not() {
        // The block is where the placeholder was, which is right. A cell is
        // parsed from an extracted copy and its offsets index that copy, so
        // it has none rather than a confident wrong one.
        let src = "Before.\n\n{% table %}\n---\n* A\n* B\n{% /table %}\n";
        let doc = parse(src, None).unwrap();
        let table = find(&doc, &|n| matches!(n.node_type, NodeType::Table)).unwrap();
        let loc = table
            .location
            .as_ref()
            .expect("the table knows where it is");
        assert!(
            src[loc.start.offset..loc.end.offset].contains("{% table %}"),
            "{:?}",
            &src[loc.start.offset..loc.end.offset]
        );
        fn no_locations(n: &Node) {
            assert!(
                n.location.is_none(),
                "{:?} should carry no position",
                n.node_type
            );
            for c in &n.children {
                no_locations(c);
            }
        }
        for child in &table.children {
            no_locations(child);
        }
    }

    #[test]
    fn an_offset_is_always_on_a_character_boundary() {
        // Nothing may hand out an offset that cannot be sliced. The interior
        // of a `{% table %}` block is parsed from an extracted copy, so its
        // nodes map to the block rather than to the cell, and the arithmetic
        // can land mid-character in prose containing an em dash.
        let src = "{% table %}\n---\n* Connected to the robot\u{2014}see below\n{% /table %}\n";
        let doc = parse(src, None).unwrap();
        fn check(n: &Node, src: &str) {
            if let Some(l) = &n.location {
                assert!(
                    src.is_char_boundary(l.start.offset) && src.is_char_boundary(l.end.offset),
                    "{:?} {}..{} is not sliceable",
                    n.node_type,
                    l.start.offset,
                    l.end.offset
                );
                assert!(l.start.offset <= l.end.offset);
                assert!(l.end.offset <= src.len());
            }
            for c in &n.children {
                check(c, src);
            }
        }
        check(&doc, src);
    }

    #[test]
    fn positions_can_still_be_turned_off() {
        let args = ParserArgs {
            file: None,
            slots: false,
            location: false,
        };
        let doc = parse("# Heading\n", Some(args)).unwrap();
        let h = find(&doc, &|n| matches!(n.node_type, NodeType::Heading)).unwrap();
        assert!(h.location.is_none());
    }

    #[test]
    fn the_file_name_travels_onto_every_node() {
        let args = ParserArgs {
            file: Some("products/mir250/intro.md".into()),
            slots: false,
            location: true,
        };
        let doc = parse("# Heading\n", Some(args)).unwrap();
        let h = find(&doc, &|n| matches!(n.node_type, NodeType::Heading)).unwrap();
        assert_eq!(
            h.location.as_ref().unwrap().file.as_deref(),
            Some("products/mir250/intro.md")
        );
    }

    // ---- syntax errors ---------------------------------------------------

    fn errors(src: &str) -> Vec<(String, usize)> {
        parse(src, None)
            .unwrap()
            .errors
            .iter()
            .map(|e| {
                (
                    e.id.clone(),
                    e.location.as_ref().map_or(0, |l| l.start.line),
                )
            })
            .collect()
    }

    #[test]
    fn an_unclosed_tag_is_reported_with_the_line_it_opened_on() {
        // It used to swallow the rest of the file in silence.
        let src = "# T\n\n{% callout %}\nbody\n";
        assert_eq!(errors(src), vec![("unclosed-tag".to_string(), 3)]);
    }

    #[test]
    fn a_close_with_nothing_open_is_reported() {
        let src = "# T\n\nText.\n\n{% /callout %}\n";
        assert_eq!(errors(src), vec![("unmatched-closing-tag".to_string(), 5)]);
    }

    #[test]
    fn tags_closed_in_the_wrong_order_are_reported() {
        // `{% /a %}` closes `a` and takes `b` with it, so `{% /b %}` then
        // has nothing to close.
        let src = "{% a %}\n{% b %}\n{% /a %}\n{% /b %}\n";
        assert_eq!(errors(src), vec![("unmatched-closing-tag".to_string(), 4)]);
    }

    #[test]
    fn a_well_formed_document_reports_nothing() {
        let src = "# T\n\n{% callout %}\nbody\n{% /callout %}\n\n{% tag id=\"x\" /%}\n";
        assert_eq!(errors(src), Vec::<(String, usize)>::new());
    }

    #[test]
    fn an_error_does_not_cost_the_rest_of_the_document() {
        // A linter wants every fault at once and a renderer wants the
        // document anyway, so a syntax error is collected, never returned.
        let src = "{% /nope %}\n\n# Still here\n";
        let doc = parse(src, None).unwrap();
        assert_eq!(doc.errors.len(), 1);
        assert!(find(&doc, &|n| matches!(n.node_type, NodeType::Heading)).is_some());
    }

    /// Walk the tree and find the first node satisfying `pred`.
    fn find<'a>(root: &'a Node, pred: &dyn Fn(&Node) -> bool) -> Option<&'a Node> {
        if pred(root) {
            return Some(root);
        }
        for child in &root.children {
            if let Some(found) = find(child, pred) {
                return Some(found);
            }
        }
        None
    }

    #[test]
    fn parses_simple_markdown_unchanged() {
        let src = "# Hello\n\nThis is **bold**.";
        let doc = parse(src, None).unwrap();
        let heading =
            find(&doc, &|n| matches!(n.node_type, NodeType::Heading)).expect("heading present");
        assert_eq!(heading.attributes.get("level"), Some(&Scalar::Number(1.0)));
        let strong =
            find(&doc, &|n| matches!(n.node_type, NodeType::Strong)).expect("strong present");
        let text =
            find(strong, &|n| matches!(n.node_type, NodeType::Text)).expect("text in strong");
        assert_eq!(
            text.attributes.get("content"),
            Some(&Scalar::String("bold".to_string()))
        );
    }

    #[test]
    fn escaped_tag_opener_stays_literal() {
        // `\{%` escapes a tag opener: it stays literal text (in prose and
        // inside inline code) and is never parsed as a tag, while a real
        // unescaped tag beside it still parses.
        let src = "a \\{% if $x %} b and `\\{% c %}`\n\n{% callout %}\nreal\n{% /callout %}";
        let doc = parse(src, None).unwrap();

        // Prose keeps the literal `{% if $x %}`.
        assert!(
            find(&doc, &|n| matches!(n.node_type, NodeType::Text)
                && matches!(n.attributes.get("content"),
                    Some(Scalar::String(s)) if s.contains("{% if $x %}")))
            .is_some(),
            "escaped opener should stay literal in prose: {doc:#?}"
        );

        // Inline code keeps the literal `{% c %}` as its content.
        let code = find(&doc, &|n| matches!(n.node_type, NodeType::Code)).expect("inline code");
        assert_eq!(
            code.attributes.get("content"),
            Some(&Scalar::String("{% c %}".to_string()))
        );

        // The escaped openers produced no tag; the real callout still did.
        assert!(
            find(&doc, &|n| n.tag.as_deref() == Some("if")
                || n.tag.as_deref() == Some("c"))
            .is_none(),
            "escaped openers must not become tags: {doc:#?}"
        );
        assert!(
            find(&doc, &|n| n.tag.as_deref() == Some("callout")).is_some(),
            "real callout tag must still parse: {doc:#?}"
        );
    }

    #[test]
    fn parses_callout_as_tag_node() {
        let src = r#"{% callout type="warning" %}
**Warn**: be careful.
{% /callout %}"#;
        let doc = parse(src, None).unwrap();

        let callout = find(&doc, &|n| {
            matches!(n.node_type, NodeType::Tag) && n.tag.as_deref() == Some("callout")
        })
        .expect("callout tag node present");

        assert_eq!(
            callout.attributes.get("type"),
            Some(&Scalar::String("warning".to_string()))
        );
        // The callout should contain the `**Warn**: be careful.` content.
        let strong = find(callout, &|n| matches!(n.node_type, NodeType::Strong))
            .expect("bold inside callout");
        let warn = find(strong, &|n| matches!(n.node_type, NodeType::Text)).expect("warn text");
        assert_eq!(
            warn.attributes.get("content"),
            Some(&Scalar::String("Warn".to_string()))
        );
    }

    #[test]
    fn parses_underscore_emphasis_with_interpolated_variable() {
        // Emphasis delimiters still wrap an inline `{% $var %}` interpolation
        // node (resolved later at transform time).
        let src = "* _{% $model %} Manual_\n";
        let doc = parse(src, None).unwrap();
        let em = find(&doc, &|n| matches!(n.node_type, NodeType::Em)).expect("em present");
        let interp = find(em, &|n| n.tag.as_deref() == Some(INTERPOLATION_TAG))
            .expect("interpolation inside em");
        assert_eq!(
            interp.expressions.get("primary").map(String::as_str),
            Some("$model")
        );
        let text = find(em, &|n| matches!(n.node_type, NodeType::Text)).expect("text in em");
        assert_eq!(
            text.attributes.get("content"),
            Some(&Scalar::String(" Manual".to_string()))
        );
    }

    #[test]
    fn parses_bold_with_interpolated_variable() {
        let src = "**{% $mir_fleet %}**\n";
        let doc = parse(src, None).unwrap();
        let strong =
            find(&doc, &|n| matches!(n.node_type, NodeType::Strong)).expect("strong present");
        let interp = find(strong, &|n| n.tag.as_deref() == Some(INTERPOLATION_TAG))
            .expect("interpolation inside strong");
        assert_eq!(
            interp.expressions.get("primary").map(String::as_str),
            Some("$mir_fleet")
        );
    }

    #[test]
    fn spaced_underscores_do_not_form_emphasis() {
        let src = "* _ {% $model %} Manual _\n";
        let doc = parse(src, None).unwrap();
        assert!(
            find(&doc, &|n| matches!(n.node_type, NodeType::Em)).is_none(),
            "spaces inside _ delimiters must prevent emphasis"
        );
    }

    #[test]
    fn indented_bold_inside_if_is_not_emphasis() {
        // Four-space indent makes this a code block in CommonMark, not bold.
        let src = r#"{% if $show %}

    **MiR Fleet**

{% /if %}"#;
        let doc = parse(src, None).unwrap();
        assert!(
            find(&doc, &|n| matches!(n.node_type, NodeType::Strong)).is_none(),
            "indented bold inside if-block is not parsed as Strong"
        );
    }

    #[test]
    fn dedented_bold_inside_if_parses_as_strong() {
        let src = r#"{% if $show %}

**MiR Fleet**

{% /if %}"#;
        let doc = parse(src, None).unwrap();
        let strong =
            find(&doc, &|n| matches!(n.node_type, NodeType::Strong)).expect("strong present");
        let text =
            find(strong, &|n| matches!(n.node_type, NodeType::Text)).expect("text in strong");
        assert_eq!(
            text.attributes.get("content"),
            Some(&Scalar::String("MiR Fleet".to_string()))
        );
    }

    #[test]
    fn captures_heading_id_sugar() {
        let src = "# Overview {% #my-overview %}";
        let doc = parse(src, None).unwrap();
        let heading =
            find(&doc, &|n| matches!(n.node_type, NodeType::Heading)).expect("heading present");
        assert_eq!(
            heading.attributes.get("id"),
            Some(&Scalar::String("my-overview".to_string()))
        );
    }

    #[test]
    fn captures_unresolved_expressions_in_node_expressions_field() {
        let src = "{% if $config.show_section %}content{% /if %}";
        let doc = parse(src, None).unwrap();
        let if_node = find(&doc, &|n| {
            matches!(n.node_type, NodeType::Tag) && n.tag.as_deref() == Some("if")
        })
        .expect("if tag node");
        // The primary expression should land in `expressions["primary"]`,
        // not in `attributes` (because it's an unresolved expression).
        assert!(!if_node.attributes.contains_key("primary"));
        assert_eq!(
            if_node.expressions.get("primary").map(String::as_str),
            Some("$config.show_section")
        );
    }

    #[test]
    fn self_closing_partial_becomes_empty_tag_node() {
        let src = r#"{% partial file="x.markdoc" /%}"#;
        let doc = parse(src, None).unwrap();
        let partial = find(&doc, &|n| {
            matches!(n.node_type, NodeType::Tag) && n.tag.as_deref() == Some("partial")
        })
        .expect("partial tag");
        assert_eq!(
            partial.attributes.get("file"),
            Some(&Scalar::String("x.markdoc".to_string()))
        );
        assert!(partial.children.is_empty());
    }

    /// Count direct children of `node` that satisfy `pred`.
    fn count_children(node: &Node, pred: &dyn Fn(&Node) -> bool) -> usize {
        node.children.iter().filter(|c| pred(c)).count()
    }

    fn is_para(n: &Node) -> bool {
        matches!(n.node_type, NodeType::Paragraph)
    }

    fn tag_named<'a>(root: &'a Node, name: &str) -> Option<&'a Node> {
        find(root, &|n| {
            matches!(n.node_type, NodeType::Tag) && n.tag.as_deref() == Some(name)
        })
    }

    #[test]
    fn block_tag_spanning_blank_line_keeps_all_content() {
        // The regression: a blank line inside a block tag used to close it
        // early, leaking the second paragraph out as a sibling.
        let src = "{% callout %}\nFirst para.\n\nSecond para.\n{% /callout %}\n";
        let doc = parse(src, None).unwrap();
        let callout = tag_named(&doc, "callout").expect("callout present");
        // Both paragraphs are now children of the callout.
        assert_eq!(
            count_children(callout, &is_para),
            2,
            "callout should own both paragraphs, tree was {doc:#?}"
        );
        // And nothing escaped to the document root besides the callout.
        assert_eq!(count_children(&doc, &is_para), 0);
    }

    #[test]
    fn block_tag_with_list_keeps_list_inside() {
        let src = "{% callout %}\nIntro.\n\n- a\n- b\n\n{% /callout %}\n";
        let doc = parse(src, None).unwrap();
        let callout = tag_named(&doc, "callout").expect("callout present");
        assert_eq!(count_children(callout, &is_para), 1);
        assert_eq!(
            count_children(callout, &|n| matches!(n.node_type, NodeType::List)),
            1,
            "list should live inside the callout"
        );
    }

    #[test]
    fn else_stays_direct_child_of_if() {
        // `{% else /%}` alone on its line must split the paragraph so it
        // remains a direct child of `{% if %}` (conditional evaluation
        // keys on that), even without blank lines around it.
        let src = "{% if $a %}\nA content\n{% else /%}\nB content\n{% /if %}\n";
        let doc = parse(src, None).unwrap();
        let if_node = tag_named(&doc, "if").expect("if present");
        assert_eq!(
            count_children(if_node, &|n| matches!(n.node_type, NodeType::Tag)
                && n.tag.as_deref() == Some("else")),
            1,
            "else must be a direct child of if, tree was {doc:#?}"
        );
        // Each branch's content is wrapped in its own paragraph.
        assert_eq!(count_children(if_node, &is_para), 2);
    }

    #[test]
    fn list_inside_callout_inside_if_keeps_following_siblings() {
        // Regression: a markdown list inside `{% callout %}` leaves
        // pending List/Item/Paragraph End events after `{% /callout %}`.
        // Those must not pop the enclosing `{% if %}`, or later callouts
        // (and `{% else / %}`) escape the conditional.
        let src = r#"{% if $x %}
{% callout type="warning" %}
Intro text.
* first item

* second item
{% /callout %}

{% callout type="warning" %}
Second callout
{% /callout %}
{% /if %}
"#;
        let doc = parse(src, None).unwrap();
        assert_eq!(
            count_children(&doc, &|n| matches!(n.node_type, NodeType::Tag)
                && n.tag.as_deref() == Some("if")),
            1,
            "exactly one top-level if, tree was {doc:#?}"
        );
        let if_node = tag_named(&doc, "if").unwrap();
        assert_eq!(
            count_children(if_node, &|n| matches!(n.node_type, NodeType::Tag)
                && n.tag.as_deref() == Some("callout")),
            2,
            "both callouts must stay inside if, tree was {doc:#?}"
        );
        assert_eq!(
            count_children(&doc, &|n| matches!(n.node_type, NodeType::Tag)
                && n.tag.as_deref() == Some("callout")),
            0,
            "no callout may leak to document root, tree was {doc:#?}"
        );
    }

    #[test]
    fn list_inside_callout_then_else_stays_inside_if() {
        let src = r#"{% if $top_module %}
{% callout type="warning" %}
If the robot transports a load that is larger than the supported size.

* Do not load the robot with loads that extend beyond supported dimensions.
{% /callout %}

{% else / %}

{% callout type="warning" %}
Else branch callout
{% /callout %}
{% /if %}
"#;
        let doc = parse(src, None).unwrap();
        let if_node = tag_named(&doc, "if").expect("if present");
        assert_eq!(
            count_children(if_node, &|n| matches!(n.node_type, NodeType::Tag)
                && n.tag.as_deref() == Some("else")),
            1,
            "else must be a direct child of if, tree was {doc:#?}"
        );
        assert_eq!(
            count_children(if_node, &|n| matches!(n.node_type, NodeType::Tag)
                && n.tag.as_deref() == Some("callout")),
            2,
            "both branch callouts must stay inside if, tree was {doc:#?}"
        );
        assert_eq!(
            count_children(&doc, &|n| matches!(n.node_type, NodeType::Tag)
                && n.tag.as_deref() == Some("else")),
            0,
            "else must not leak to document root, tree was {doc:#?}"
        );
    }

    #[test]
    fn inline_tag_sharing_a_line_stays_inline() {
        // A tag with text on the same line is inline and must NOT be
        // lifted to block level.
        let src = "See {% tagref id=\"x\" /%} for details.\n";
        let doc = parse(src, None).unwrap();
        // The tagref sits inside the single paragraph, not at doc root.
        assert_eq!(count_children(&doc, &is_para), 1);
        let para = doc.children.iter().find(|c| is_para(c)).unwrap();
        assert_eq!(
            count_children(para, &|n| matches!(n.node_type, NodeType::Tag)
                && n.tag.as_deref() == Some("tagref")),
            1
        );
    }

    #[test]
    fn standalone_self_closing_tag_lifts_to_block() {
        // A self-closing tag alone in its own paragraph becomes a
        // block-level node with the empty paragraph dropped.
        let src = "Before.\n\n{% partial file=\"x.markdoc\" /%}\n\nAfter.\n";
        let doc = parse(src, None).unwrap();
        let partial = tag_named(&doc, "partial").expect("partial present");
        // partial is a direct child of the document, between two paragraphs.
        assert_eq!(
            count_children(&doc, &|n| matches!(n.node_type, NodeType::Tag)
                && n.tag.as_deref() == Some("partial")),
            1
        );
        assert!(partial.children.is_empty());
        assert_eq!(count_children(&doc, &is_para), 2);
    }

    #[test]
    fn frontmatter_interpolation_parses_to_a_node() {
        // `{% $markdoc.frontmatter.title %}` is captured as an interpolation
        // node (resolved against the Context at transform time), not
        // substituted during parse.
        let src = "---\ntitle: Hello\n---\n\n# {% $markdoc.frontmatter.title %}";
        let doc = parse(src, None).unwrap();
        let node = find(&doc, &|n| n.tag.as_deref() == Some(INTERPOLATION_TAG))
            .expect("interpolation node present");
        assert_eq!(
            node.expressions.get("primary").map(String::as_str),
            Some("$markdoc.frontmatter.title")
        );
    }

    #[test]
    fn raw_html_renders_as_literal_text_but_comments_stay_hidden() {
        // Markdoc has no raw-HTML support, but an unrecognised tag-like
        // token (a placeholder such as `<insert …>`) must survive as
        // literal text instead of being silently dropped.
        let doc = parse("Product: <insert list of robots and HW versions>\n", None).unwrap();
        let kept = find(&doc, &|n| {
            matches!(n.node_type, NodeType::Text)
                && matches!(
                    n.attributes.get("content"),
                    Some(Scalar::String(s)) if s.contains("<insert list of robots and HW versions>")
                )
        });
        assert!(
            kept.is_some(),
            "raw `<insert …>` should survive as literal text, tree was {doc:#?}"
        );

        // HTML comments, by contrast, must stay hidden — their content
        // must never leak into the rendered text.
        let doc2 = parse("Before <!-- secret note --> after.\n", None).unwrap();
        let leaked = find(&doc2, &|n| {
            matches!(n.node_type, NodeType::Text)
                && matches!(
                    n.attributes.get("content"),
                    Some(Scalar::String(s)) if s.contains("secret")
                )
        });
        assert!(
            leaked.is_none(),
            "HTML comment content must not render, tree was {doc2:#?}"
        );
    }

    #[test]
    fn pipe_table_preserves_column_alignment() {
        // The `:--`/`:-:`/`--:` delimiter row must survive as an `align`
        // attribute on the table node so renderers can align columns.
        let src = "| L | C | R |\n|:--|:-:|--:|\n| a | b | c |\n";
        let doc = parse(src, None).unwrap();
        let table = find(&doc, &|n| matches!(n.node_type, NodeType::Table)).expect("table present");
        match table.attributes.get("align") {
            Some(Scalar::Array(a)) => {
                let names: Vec<&str> = a
                    .iter()
                    .filter_map(|s| match s {
                        Scalar::String(x) => Some(x.as_str()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(names, vec!["left", "center", "right"]);
            }
            other => panic!("expected an align array, got {other:?}"),
        }

        // A table with no explicit alignment carries no `align` attribute.
        let plain = parse("| A | B |\n|---|---|\n| a | b |\n", None).unwrap();
        let t2 = find(&plain, &|n| matches!(n.node_type, NodeType::Table)).expect("table");
        assert!(!t2.attributes.contains_key("align"));
    }

    #[test]
    fn indented_if_inside_ordered_list_keeps_one_list() {
        // PDF preprocess keeps list-continuation indent on `{% if %}` so
        // CommonMark does not terminate the ordered list between steps.
        let src = r#"1. First step.

2. Second step.
   {% if not($top_module) %}
   {% image id="aaa" /%}
   {% else / %}
   {% image id="bbb" /%}
   {% /if %}

3. Third step.
"#;
        let doc = parse(src, None).unwrap();
        assert_eq!(
            count_children(&doc, &|n| matches!(n.node_type, NodeType::List)),
            1,
            "expected a single ordered list, tree was {doc:#?}"
        );
        let list = find(&doc, &|n| matches!(n.node_type, NodeType::List)).unwrap();
        assert_eq!(
            count_children(list, &|n| matches!(n.node_type, NodeType::Item)),
            3,
            "expected three list items, tree was {doc:#?}"
        );
        let second = list
            .children
            .iter()
            .filter(|n| matches!(n.node_type, NodeType::Item))
            .nth(1)
            .expect("second item");
        assert_eq!(
            count_children(second, &|n| matches!(n.node_type, NodeType::Tag)
                && n.tag.as_deref() == Some("if")),
            1,
            "conditional must stay inside the second list item, tree was {doc:#?}"
        );
    }
}
