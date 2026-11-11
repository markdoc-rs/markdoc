//! Port of upstream Markdoc v0.5.11's `src/transforms/table.ts`.
//!
//! Owner: the parser agent.
//!
//! A `{% table %}` tag's children (lists for rows, `---` between them) become a `table`
//! node with a `thead` (the first list, as a row of `th`) and a `tbody` (the other lists,
//! as rows of `td`; conditional tags among them kept as row wrappers). Anything else at row
//! level is dropped with a `table-syntax` error on the tag.
//!
//! As upstream's, the transform runs during a walk that yields a node before descending
//! into it, so a table nested in a cell is transformed too, and what the transform drops
//! is never visited. Upstream's walk is a recursive generator; this one keeps its own stack
//! on the heap, in the same order, so it costs no stack at any depth, and it returns
//! upstream's `RangeError` where V8's stack runs out in upstream's walk: at a node deeper
//! than [`crate::next::parser::MAX_AST_DEPTH`] (from the top of a thread; what is left of
//! [`crate::depth`]'s budget below a caller that charges it).
//!
//! The `table`, `thead` and `tbody` nodes are upstream's `new Ast.Node(...)`: no `lines`, no
//! `location`. This crate's addition: they get a [`Span`], so that every node of the AST has
//! one: `thead` and `tbody` the span of the rows they hold (the tag's when they hold none),
//! `table` from its tag's `{%` to the end of its last row.

use crate::ast::Node;
use crate::next::parser::{AST_COST, stack_overflow};
use crate::types::{Location, NodeType, Result, Span, ValidationError, ValidationLevel};
use indexmap::IndexMap;

/// `convertToRow(node, cellType)`.
fn convert_to_row(node: &mut Node, cell_type: NodeType) {
    node.node_type = NodeType::Tr;
    node.attributes = IndexMap::new();
    node.attributes_prototype = None;

    for cell in &mut node.children {
        cell.node_type = cell_type.clone();
    }
}

/// `isConditionalTag(node, conditionalTags)`.
fn is_conditional_tag(node: &Node, conditional_tags: &[String]) -> bool {
    node.node_type == NodeType::Tag
        && node
            .tag
            .as_ref()
            .is_some_and(|tag| !tag.is_empty() && conditional_tags.contains(tag))
}

/// `isComment(node)`.
fn is_comment(node: &Node) -> bool {
    node.node_type == NodeType::Comment
        || (node.node_type == NodeType::Tag && node.tag.as_deref() == Some("comment"))
}

/// `unexpectedNodeError(node)`: `location` is the node's (left out when it has none).
fn unexpected_node_error(node: &Node) -> ValidationError {
    let tag = match node.tag.as_deref() {
        Some(tag) if !tag.is_empty() => format!(" {tag}"),
        _ => String::new(),
    };
    ValidationError {
        id: "table-syntax".into(),
        level: ValidationLevel::Critical,
        message: format!(
            "Found {}{tag} where a list was expected. Make sure all content inside table cells is indented.",
            node.node_type.as_str()
        ),
        location: node.location.as_ref().map(Location::to_json),
        raw: None,
        span: node.span.clone(),
    }
}

/// `transform(document, conditionalTags = ['if'])`. `Err` (`RangeError`) for a tree deeper
/// than [`crate::next::parser::MAX_AST_DEPTH`], once the walk reaches a node below it
/// (`document` is then left in pieces).
pub fn transform(document: &mut Node, conditional_tags: Option<&[String]>) -> Result<()> {
    let default = ["if".to_string()];
    let conditional_tags = conditional_tags.unwrap_or(&default);
    walk(document, conditional_tags)
}

/// `for (const node of document.walk())`: each slot, then each child, yielded (and so
/// transformed) before its own walk, which then sees what the transform made of it.
///
/// `walk()` is `for (const child of [...Object.values(this.slots), ...this.children]) {
/// yield child; yield* child.walk(); }`. Here each node being walked is a [`Frame`] on a
/// stack, holding the node's slots and children (taken out of it, so that the stack owns
/// what it walks and the node stays where it is); they go back into the node when its walk
/// ends. The document's walk is the bottom frame (the document itself is not yielded), so
/// a node's depth is the number of frames under its own.
fn walk(document: &mut Node, conditional_tags: &[String]) -> Result<()> {
    // How deep a node may be: MAX_AST_DEPTH from the top of a thread, what is left of the
    // depth budget (`crate::depth`) below a caller that charges it.
    let limit = usize::try_from(crate::depth::left() / AST_COST).unwrap_or(usize::MAX);
    let mut stack = vec![Frame::open(document)];
    loop {
        let depth = stack.len();
        let top = stack.last_mut().expect("the document's frame");
        if let Some(node) = top.next() {
            // `yield child`
            transform_node(node, conditional_tags);
            // `yield* child.walk()`, which starts from what the transform left, and where
            // V8's stack runs out below MAX_AST_DEPTH.
            if depth > limit {
                return Err(stack_overflow());
            }
            let frame = Frame::open(node);
            stack.push(frame);
            continue;
        }
        let done = stack.pop().expect("the frame just looked at");
        match stack.last_mut() {
            Some(parent) => done.close(parent.current()),
            None => {
                done.close(document);
                return Ok(());
            }
        }
    }
}

/// A node whose walk is under way (see [`walk`]): its slots and children, and how many of
/// them have been yielded.
struct Frame {
    slots: IndexMap<String, Node>,
    children: Vec<Node>,
    yielded: usize,
}

impl Frame {
    /// The walk of `node` begins: `[...Object.values(this.slots), ...this.children]`.
    fn open(node: &mut Node) -> Frame {
        Frame {
            slots: std::mem::take(&mut node.slots),
            children: std::mem::take(&mut node.children),
            yielded: 0,
        }
    }

    /// The next node to yield: the slots first, then the children.
    fn next(&mut self) -> Option<&mut Node> {
        let i = self.yielded;
        self.yielded += 1;
        let slots = self.slots.len();
        match self.slots.get_index_mut(i) {
            Some((_, slot)) => Some(slot),
            None => self.children.get_mut(i - slots),
        }
    }

    /// The node yielded last, whose own walk is under way.
    fn current(&mut self) -> &mut Node {
        let i = self.yielded - 1;
        let slots = self.slots.len();
        match self.slots.get_index_mut(i) {
            Some((_, slot)) => slot,
            None => &mut self.children[i - slots],
        }
    }

    /// The walk of `node` ends: its slots and children go back into it.
    fn close(self, node: &mut Node) {
        node.slots = self.slots;
        node.children = self.children;
    }
}

/// The loop body, for one node.
fn transform_node(node: &mut Node, conditional_tags: &[String]) {
    if node.node_type != NodeType::Tag || node.tag.as_deref() != Some("table") {
        return;
    }

    // `const [first, ...rest] = node.children; if (!first || first.type === 'table') continue;`
    match node.children.first() {
        None => return,
        Some(first) if first.node_type == NodeType::Table => return,
        Some(_) => {}
    }
    let mut rest = std::mem::take(&mut node.children);
    let mut first = rest.remove(0);

    let mut thead = Node::new(NodeType::Thead, IndexMap::new(), Vec::new(), None);
    let mut tbody = Node::new(NodeType::Tbody, IndexMap::new(), Vec::new(), None);

    if first.node_type == NodeType::List {
        convert_to_row(&mut first, NodeType::Th);
        thead.push(first);
    }

    for mut row in rest {
        // Convert lists to rows with special-case support for conditionals
        // When a conditional is encountered, convert all of its top-level lists to rows
        if row.node_type == NodeType::List {
            convert_to_row(&mut row, NodeType::Td);
        } else if is_conditional_tag(&row, conditional_tags) {
            let mut children = Vec::new();

            for mut child in std::mem::take(&mut row.children) {
                // Replace children and skip HRs in order to support conditionals with multiple rows
                if child.node_type == NodeType::Hr {
                    continue;
                }
                if child.node_type == NodeType::List {
                    convert_to_row(&mut child, NodeType::Td);
                } else if is_comment(&child)
                    || child.tag.as_deref() == Some("else")
                    || is_conditional_tag(&child, conditional_tags)
                {
                    // Allow structural tags: else, nested conditionals, and comments
                } else {
                    row.errors.push(unexpected_node_error(&child));
                    continue;
                }
                children.push(child);
            }

            row.children = children;
        } else if row.node_type != NodeType::Hr && !is_comment(&row) {
            node.errors.push(unexpected_node_error(&row));
            continue;
        } else {
            continue;
        }
        tbody.push(row);
    }

    // `new Ast.Node('table', node.attributes, [thead, tbody])`: upstream shares the
    // attributes object between the tag and the table; nothing changes it afterwards.
    locate(&mut thead, node.span.as_ref(), false);
    locate(&mut tbody, node.span.as_ref(), false);
    let mut table = Node::new(
        NodeType::Table,
        node.attributes.clone(),
        vec![thead, tbody],
        None,
    );
    table.attributes_prototype = node.attributes_prototype.clone();
    locate(&mut table, node.span.as_ref(), true);

    node.children = vec![table];
}

/// This crate's: a made node's [`Span`], from its first child's start to its last child's
/// end, or `tag`'s when its children have none. The table itself starts where its tag does
/// (so its first line is the `{% table %}` line, as the old parser's list-table node's is).
fn locate(node: &mut Node, tag: Option<&Span>, from_tag: bool) {
    let Some(tag) = tag else {
        return;
    };
    let mut span: Option<Span> = from_tag.then(|| tag.clone());
    for l in node.children.iter().filter_map(|c| c.span.as_ref()) {
        match &mut span {
            None => span = Some(l.clone()),
            Some(span) => {
                if l.start.offset < span.start.offset {
                    span.start = l.start.clone();
                }
                if l.end.offset > span.end.offset {
                    span.end = l.end.clone();
                }
            }
        }
    }
    node.span = Some(span.unwrap_or_else(|| tag.clone()));
}
