//! Markdoc list-syntax `{% table %}` support.
//!
//! markdoc segments `{% %}` tags and hands the rest to pulldown-cmark, which
//! mangles a list-table's `---` row separators (into thematic breaks / setext
//! headings) and `*` cells. So we intercept `{% table %}` blocks *before*
//! tokenization: [`extract_list_tables`] parses each block's raw inner text
//! into a proper `Table` node and leaves a placeholder sentinel in its place,
//! which [`splice_list_tables`] swaps back once the surrounding document has
//! been built.
//!
//! Rows are separated by top-level `---`; each `* item` in a row is a cell,
//! parsed recursively as Markdown so cells may hold rich content (paragraphs,
//! code, lists). The first row is the header unless the table starts with
//! `---`.
//!
//! `{% if %}` / `{% else %}` / `{% /if %}` at row boundaries are preserved as
//! Tag nodes wrapping the enclosed `Tr`s so [`crate::evaluate_conditionals`]
//! can splice or drop them after parse (the same path as conditionals outside
//! tables). Without this, `---` inside an `if` would always emit rows.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

use crate::ast::Node;
use crate::sourcemap::SourceMap;
use crate::tag_parser::{self, AttrValue, TagAttrs};
use crate::types::{NodeType, Scalar};

/// Placeholder sentinels for an extracted table, distinct from the tag
/// sentinels in `tag_parser`: `\u{E002}<index>\u{E003}`.
const TABLE_OPEN: char = '\u{E002}';
const TABLE_CLOSE: char = '\u{E003}';

/// One parsed table cell: its content (the children of its `Td`/`Th`) plus
/// any `colspan` / `rowspan` / `align` annotations stripped from its source.
struct Cell {
    content: Vec<Node>,
    attrs: HashMap<String, Scalar>,
}
/// One row: its cells, in column order.
type Row = Vec<Cell>;

/// A structural unit inside a list-table body: a raw row block, an `if`
/// group, or an `else` branch marker.
enum TableItem {
    /// Raw text of one row (the `* cell` list between `---` separators).
    Row(String),
    /// `{% if … %}` … `{% /if %}` wrapping further items (rows / else / nested if).
    If {
        attrs: TagAttrs,
        children: Vec<TableItem>,
    },
    /// Self-closing `{% else … /%}` branch marker inside an `if`.
    Else { attrs: TagAttrs },
}

fn open_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // `{% table %}` / `{% table attrs %}` — an opening tag (not `/table`).
    // Capture group 1 is the attribute body (may be empty / whitespace).
    RE.get_or_init(|| Regex::new(r"\{%\s*table\b([^%]*)%\}").unwrap())
}

fn close_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{%\s*/\s*table\s*%\}").unwrap())
}

fn if_open_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\{%\s*if\b([^%]*)%\}$").unwrap())
}

fn if_close_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\{%\s*/\s*if\s*%\}$").unwrap())
}

fn else_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Self-closing `{% else /%}` or `{% else $cond /%}`.
    RE.get_or_init(|| Regex::new(r"^\{%\s*else\b([^%]*)/\s*%\}$").unwrap())
}

/// Does a `{% table %}` block use the Markdoc list-syntax (its first
/// non-blank line is a `*` cell or a `---` separator) rather than wrapping a
/// pipe table (which starts with `|`)? Only list-syntax blocks are parsed
/// here; pipe-table wrappers fall through to the normal tag pipeline.
fn is_list_syntax(inner: &str) -> bool {
    for line in inner.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        return t == "*" || t == "---" || t.starts_with("* ") || t.starts_with("*\t");
    }
    false
}

/// Replace each `{% table %}…{% /table %}` block with a placeholder sentinel
/// and return the parsed `Table` nodes (indexed by the sentinel). The inner
/// text is parsed before pulldown-cmark sees it, so `---`/`*` survive.
/// Opening-tag attributes (e.g. `column_weights="1 2"`) are copied onto the
/// resulting `Table` node so the PDF renderer can honour them.
/// Lift `{% table %}` list-syntax blocks out, recording where each went.
///
/// The map is what lets a diagnostic inside -- or after -- a table still point
/// at the right byte: a whole block collapses to a three-character
/// placeholder, so everything downstream of one is displaced by however long
/// the table was.
pub fn extract_list_tables_mapped(content: &str) -> (String, Vec<Node>, SourceMap) {
    let mut out = String::with_capacity(content.len());
    let mut tables: Vec<Node> = Vec::new();
    let mut map = SourceMap::identity();
    let mut pos = 0;
    while let Some(m) = open_re().captures(&content[pos..]) {
        let full = m.get(0).unwrap();
        let open_start = pos + full.start();
        let open_end = pos + full.end();
        let open_attrs = m.get(1).map(|g| g.as_str()).unwrap_or("");
        match close_re().find(&content[open_end..]) {
            Some(c) => {
                let close_end = open_end + c.end();
                let inner = &content[open_end..open_end + c.start()];
                if is_list_syntax(inner) {
                    out.push_str(&content[pos..open_start]);
                    let placeholder_start = out.len();
                    let idx = tables.len();
                    tables.push(parse_list_table(inner, open_attrs));
                    out.push(TABLE_OPEN);
                    out.push_str(&idx.to_string());
                    out.push(TABLE_CLOSE);
                    map.replace(placeholder_start..out.len(), open_start..close_end);
                } else {
                    // Not list-syntax (e.g. a pipe table wrapped in
                    // `{% table … %}` for styling) — leave it verbatim for
                    // the normal tag pipeline + pipe-table renderer.
                    out.push_str(&content[pos..close_end]);
                }
                pos = close_end;
            }
            None => {
                // Unbalanced open — leave it verbatim and stop scanning.
                out.push_str(&content[pos..open_end]);
                pos = open_end;
            }
        }
    }
    out.push_str(&content[pos..]);
    (out, tables, map)
}

/// Without the map, for callers that do not need positions.
pub fn extract_list_tables(content: &str) -> (String, Vec<Node>) {
    let (out, tables, _) = extract_list_tables_mapped(content);
    (out, tables)
}

/// Walk `node` and replace each placeholder (a paragraph or text node holding
/// a `\u{E002}N\u{E003}` sentinel) with `tables[N]`.
pub fn splice_list_tables(node: &mut Node, tables: &[Node]) {
    let mut i = 0;
    while i < node.children.len() {
        if let Some(idx) = placeholder_index(&node.children[i]) {
            if let Some(table) = tables.get(idx) {
                // The placeholder stands exactly where the `{% table %}`
                // block is in the file, so the table inherits its position.
                let at = node.children[i].location.clone();
                let lines = node.children[i].lines.clone();
                let mut table = table.clone();
                table.location = at;
                table.lines = lines;
                // The interior is a different matter. Cells are parsed from
                // an extracted copy of the block, one non-contiguous piece
                // at a time, so a cell's offsets are relative to that copy
                // and index the file at some unrelated place -- which is
                // worse than having none, because an editor would highlight
                // the wrong text with perfect confidence. Until `parse_row`
                // tracks where each cell began, they have none.
                for child in &mut table.children {
                    clear_locations(child);
                }
                node.children[i] = table;
            }
        } else {
            splice_list_tables(&mut node.children[i], tables);
        }
        i += 1;
    }
}

fn clear_locations(node: &mut Node) {
    node.location = None;
    node.lines.clear();
    for child in &mut node.children {
        clear_locations(child);
    }
}

/// If `node` is a placeholder (a paragraph wrapping a single placeholder text,
/// or a placeholder text node), return its table index.
fn placeholder_index(node: &Node) -> Option<usize> {
    let text = match &node.node_type {
        NodeType::Paragraph if node.children.len() == 1 => text_content(&node.children[0])?,
        NodeType::Text => text_content(node)?,
        _ => return None,
    };
    let t = text.trim();
    let inner = t.strip_prefix(TABLE_OPEN)?.strip_suffix(TABLE_CLOSE)?;
    inner.parse::<usize>().ok()
}

fn text_content(node: &Node) -> Option<&str> {
    if matches!(node.node_type, NodeType::Text)
        && let Some(Scalar::String(s)) = node.attributes.get("content")
    {
        Some(s)
    } else {
        None
    }
}

/// Parse a `{% table %}` block's inner text into a `Table` node.
/// `open_attrs` is the attribute body from the opening tag (e.g.
/// ` column_weights="1 2"`); literals are merged onto the table node so
/// PDF styling overrides survive list-syntax extraction.
fn parse_list_table(inner: &str, open_attrs: &str) -> Node {
    let items = parse_table_items(inner);
    // Header-less when the table begins with `---` (first item is an empty
    // row produced by that leading separator) or when there are no items.
    let leading_sep = matches!(items.first(), Some(TableItem::Row(s)) if s.trim().is_empty());
    let body_start = if leading_sep || items.is_empty() {
        0
    } else {
        // First item must be a plain row to act as the header.
        match items.first() {
            Some(TableItem::Row(_)) => 1,
            _ => 0,
        }
    };

    let mut children = Vec::new();
    let mut table_attrs = HashMap::new();
    // Opening-tag attrs first so header-derived `align` can still override
    // if an author ever sets both (align from cells is the source of truth
    // for column alignment).
    merge_open_attrs(&mut table_attrs, open_attrs);
    if body_start == 1
        && let TableItem::Row(block) = &items[0]
    {
        {
            let header = parse_row(block);
            if let Some(aligns) = column_aligns(&header) {
                table_attrs.insert("align".to_string(), aligns);
            }
            let head_cells = make_cells(&header, true);
            let tr = Node::new(NodeType::Tr, HashMap::new(), head_cells, None);
            children.push(Node::new(NodeType::Thead, HashMap::new(), vec![tr], None));
        }
    }

    let body_nodes = table_items_to_nodes(&items[body_start..]);
    if !body_nodes.is_empty() {
        children.push(Node::new(NodeType::Tbody, HashMap::new(), body_nodes, None));
    }

    Node::new(NodeType::Table, table_attrs, children, None)
}

/// Copy literal attributes from a `{% table … %}` opening tag onto
/// `attrs`. Expression-valued attrs are ignored here (list-table
/// extraction runs before an evaluation context exists); authors should
/// use literals such as `column_weights="1 2"`.
fn merge_open_attrs(attrs: &mut HashMap<String, Scalar>, open_attrs: &str) {
    let parsed = tag_parser::parse_attrs(open_attrs);
    for (key, value) in parsed.named {
        if let AttrValue::Literal(s) = value {
            attrs.insert(key, s);
        }
    }
}

/// Convert structural table items into `Tr` / `if` / `else` nodes for the
/// tbody (or nested inside an `if`).
fn table_items_to_nodes(items: &[TableItem]) -> Vec<Node> {
    let mut out = Vec::new();
    for item in items {
        match item {
            TableItem::Row(block) => {
                if block.trim().is_empty() {
                    continue;
                }
                let cells = parse_row(block);
                if cells.is_empty() {
                    continue;
                }
                let tds = make_cells(&cells, false);
                out.push(Node::new(NodeType::Tr, HashMap::new(), tds, None));
            }
            TableItem::If { attrs, children } => {
                out.push(make_conditional_tag(
                    "if",
                    attrs,
                    table_items_to_nodes(children),
                ));
            }
            TableItem::Else { attrs } => {
                out.push(make_conditional_tag("else", attrs, Vec::new()));
            }
        }
    }
    out
}

fn make_conditional_tag(name: &str, attrs: &TagAttrs, children: Vec<Node>) -> Node {
    let mut attributes: HashMap<String, Scalar> = HashMap::new();
    let mut expressions: HashMap<String, String> = HashMap::new();
    if let Some(primary) = &attrs.primary {
        match primary {
            AttrValue::Literal(s) => {
                attributes.insert("primary".to_string(), s.clone());
            }
            AttrValue::Expression(src) => {
                expressions.insert("primary".to_string(), src.clone());
            }
        }
    }
    for (key, value) in &attrs.named {
        match value {
            AttrValue::Literal(s) => {
                attributes.insert(key.clone(), s.clone());
            }
            AttrValue::Expression(src) => {
                expressions.insert(key.clone(), src.clone());
            }
        }
    }
    let mut node = Node::new(NodeType::Tag, attributes, children, Some(name.to_string()));
    node.expressions = expressions;
    node
}

/// Split a table's inner text into [`TableItem`]s, honouring row `---`
/// separators and row-boundary `{% if %}` / `{% else %}` / `{% /if %}`.
fn parse_table_items(inner: &str) -> Vec<TableItem> {
    let lines: Vec<&str> = inner.lines().collect();
    let (items, _) = parse_table_items_from(&lines, 0, false);
    items
}

/// Parse items starting at `start`. When `inside_if` is true, a top-level
/// `{% /if %}` ends the current group and returns. Returns `(items, next_index)`.
fn parse_table_items_from(
    lines: &[&str],
    start: usize,
    inside_if: bool,
) -> (Vec<TableItem>, usize) {
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut in_fence = false;
    let mut i = start;

    let flush_row = |cur: &mut String, items: &mut Vec<TableItem>| {
        items.push(TableItem::Row(std::mem::take(cur)));
    };

    while i < lines.len() {
        let line = lines[i];
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
        }
        let trimmed = line.trim();
        if !in_fence {
            if let Some(caps) = if_open_re().captures(trimmed) {
                flush_row(&mut cur, &mut items);
                let attrs = tag_parser::parse_attrs(caps.get(1).map(|m| m.as_str()).unwrap_or(""));
                let (children, next) = parse_table_items_from(lines, i + 1, true);
                items.push(TableItem::If { attrs, children });
                i = next;
                continue;
            }
            if inside_if && if_close_re().is_match(trimmed) {
                flush_row(&mut cur, &mut items);
                return (items, i + 1);
            }
            if inside_if && let Some(caps) = else_re().captures(trimmed) {
                flush_row(&mut cur, &mut items);
                let attrs = tag_parser::parse_attrs(caps.get(1).map(|m| m.as_str()).unwrap_or(""));
                items.push(TableItem::Else { attrs });
                i += 1;
                continue;
            }
            if trimmed == "---" {
                flush_row(&mut cur, &mut items);
                i += 1;
                continue;
            }
        }
        cur.push_str(line);
        cur.push('\n');
        i += 1;
    }
    flush_row(&mut cur, &mut items);
    (items, i)
}

/// Build the table `align` attribute (an array of `left`/`center`/`right`/``)
/// from the header row's per-cell `align`. `None` when no column is aligned.
fn column_aligns(header: &[Cell]) -> Option<Scalar> {
    let names: Vec<Scalar> = header
        .iter()
        .map(|c| {
            let name = match c.attrs.get("align") {
                Some(Scalar::String(s)) => s.clone(),
                _ => String::new(),
            };
            Scalar::String(name)
        })
        .collect();
    let any = names
        .iter()
        .any(|s| matches!(s, Scalar::String(x) if !x.is_empty()));
    any.then_some(Scalar::Array(names))
}

fn annotation_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"\{%\s*(colspan|rowspan|align)\s*=\s*("[^"]*"|[^\s%}]+)\s*%\}"#).unwrap()
    })
}

/// Strip `{% colspan=N %}` / `{% rowspan=N %}` / `{% align="…" %}` annotations
/// from a row block, returning the cleaned text and `(cell_index, key, value)`
/// for each — the cell index being the `*` item the annotation trailed.
fn strip_annotations(block: &str) -> (String, Vec<(usize, String, Scalar)>) {
    let re = annotation_re();
    let mut found = Vec::new();
    let mut clean = String::with_capacity(block.len());
    let mut last = 0;
    for caps in re.captures_iter(block) {
        let m = caps.get(0).unwrap();
        if let Some(idx) = cell_index_at(block, m.start()) {
            let key = caps[1].to_string();
            let value = parse_attr_value(&key, &caps[2]);
            found.push((idx, key, value));
        }
        clean.push_str(&block[last..m.start()]);
        last = m.end();
    }
    clean.push_str(&block[last..]);
    (clean, found)
}

/// The 0-based index of the `*` cell `byte_pos` falls in: the number of
/// top-level (column-0) `*` markers starting at or before it, minus one.
fn cell_index_at(block: &str, byte_pos: usize) -> Option<usize> {
    let mut count = 0usize;
    let mut offset = 0usize;
    for line in block.split_inclusive('\n') {
        if offset >= byte_pos {
            break;
        }
        if line.starts_with('*') {
            count += 1;
        }
        offset += line.len();
    }
    count.checked_sub(1)
}

/// Parse an annotation value: `colspan`/`rowspan` as a number, `align` (and
/// anything else) as a string, with surrounding quotes removed.
fn parse_attr_value(key: &str, raw: &str) -> Scalar {
    let v = raw.trim().trim_matches('"');
    match key {
        "colspan" | "rowspan" => v
            .parse::<f64>()
            .map(Scalar::Number)
            .unwrap_or_else(|_| Scalar::String(v.to_string())),
        _ => Scalar::String(v.to_string()),
    }
}

/// Build `Th`/`Td` cell nodes from a row's parsed cells, carrying any
/// `colspan` / `rowspan` / `align` attributes.
fn make_cells(cells: &[Cell], header: bool) -> Vec<Node> {
    let nt = if header { NodeType::Th } else { NodeType::Td };
    cells
        .iter()
        .map(|c| Node::new(nt.clone(), c.attrs.clone(), c.content.clone(), None))
        .collect()
}

/// Parse one row block (a Markdown bulleted list) into its cells. Cell
/// annotations (`{% colspan=N %}` etc.) are stripped first — markdoc would
/// otherwise parse them as malformed tags that swallow the following cells —
/// then re-attached to the cell they trailed.
fn parse_row(block: &str) -> Row {
    let (clean, annotations) = strip_annotations(block);
    let Ok(doc) = crate::parse(&clean, None) else {
        return Vec::new();
    };
    let Some(list) = find_list(&doc) else {
        return Vec::new();
    };
    let mut cells: Row = list
        .children
        .iter()
        .filter(|n| matches!(n.node_type, NodeType::Item))
        .map(|item| Cell {
            content: item.children.clone(),
            attrs: HashMap::new(),
        })
        .collect();
    for (idx, key, value) in annotations {
        if let Some(cell) = cells.get_mut(idx) {
            cell.attrs.insert(key, value);
        }
    }
    cells
}

/// Find the first `List` node anywhere in the tree.
fn find_list(node: &Node) -> Option<&Node> {
    if matches!(node.node_type, NodeType::List) {
        return Some(node);
    }
    node.children.iter().find_map(find_list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conditionals::evaluate_conditionals;
    use crate::types::Context;

    fn find_node<'a>(node: &'a Node, nt: &NodeType) -> Option<&'a Node> {
        if &node.node_type == nt {
            return Some(node);
        }
        node.children.iter().find_map(|c| find_node(c, nt))
    }

    fn count_nodes(node: &Node, nt: &NodeType) -> usize {
        let here = usize::from(&node.node_type == nt);
        here + node
            .children
            .iter()
            .map(|c| count_nodes(c, nt))
            .sum::<usize>()
    }

    fn count_text(node: &Node, needle: &str) -> usize {
        let mut n = 0;
        if matches!(node.node_type, NodeType::Text)
            && let Some(Scalar::String(s)) = node.attributes.get("content")
            && s.contains(needle)
        {
            n += 1;
        }
        for c in &node.children {
            n += count_text(c, needle);
        }
        n
    }

    fn has_tag(node: &Node, name: &str) -> bool {
        if matches!(node.node_type, NodeType::Tag) && node.tag.as_deref() == Some(name) {
            return true;
        }
        node.children.iter().any(|c| has_tag(c, name))
    }

    fn ctx_with(pairs: &[(&str, Scalar)]) -> Context {
        let mut c = Context::new();
        for (k, v) in pairs {
            c = c.with_variable(*k, v.clone());
        }
        c
    }

    #[test]
    fn basic_list_table_has_header_and_body() {
        let doc = crate::parse(
            "{% table %}\n* H1\n* H2\n---\n* a\n* b\n{% /table %}\n",
            None,
        )
        .unwrap();
        let table = find_node(&doc, &NodeType::Table).expect("table node");
        assert!(find_node(table, &NodeType::Thead).is_some(), "has a header");
        assert_eq!(count_nodes(table, &NodeType::Th), 2, "two header cells");
        assert_eq!(count_nodes(table, &NodeType::Td), 2, "two body cells");
    }

    #[test]
    fn header_less_list_table_has_no_thead() {
        let doc = crate::parse(
            "{% table %}\n---\n* a\n* b\n---\n* c\n* d\n{% /table %}\n",
            None,
        )
        .unwrap();
        let table = find_node(&doc, &NodeType::Table).expect("table node");
        assert!(find_node(table, &NodeType::Thead).is_none(), "no header");
        assert_eq!(count_nodes(table, &NodeType::Th), 0);
        assert_eq!(count_nodes(table, &NodeType::Td), 4, "four body cells");
    }

    fn find_tag<'a>(node: &'a Node, name: &str) -> Option<&'a Node> {
        if matches!(node.node_type, NodeType::Tag) && node.tag.as_deref() == Some(name) {
            return Some(node);
        }
        node.children.iter().find_map(|c| find_tag(c, name))
    }

    #[test]
    fn pipe_table_wrapper_is_not_intercepted() {
        // `{% table … %}` wrapping a PIPE table (a styling wrapper) must not be
        // parsed as a list-table — it stays a Tag with the pipe table inside.
        let src = "{% table borders=\"grid\" %}\n| A | B |\n|---|---|\n| x | y |\n{% /table %}\n";
        let doc = crate::parse(src, None).unwrap();
        assert!(
            find_tag(&doc, "table").is_some(),
            "the table styling wrapper survived as a tag"
        );
        let table = find_node(&doc, &NodeType::Table).expect("inner pipe table");
        assert!(
            count_nodes(table, &NodeType::Td) >= 2,
            "pipe table parsed normally (not mis-parsed as an empty list-table)"
        );
    }

    #[test]
    fn rich_cell_keeps_block_content() {
        // A cell holding a fenced code block must survive as a Fence node.
        let src = "{% table %}\n* H\n---\n*\n  ```\n  code\n  ```\n* plain\n{% /table %}\n";
        let doc = crate::parse(src, None).unwrap();
        let table = find_node(&doc, &NodeType::Table).expect("table node");
        assert!(
            find_node(table, &NodeType::Fence).is_some(),
            "code block in a cell is preserved, tree was {table:#?}"
        );
    }

    fn has_attr_number(node: &Node, nt: &NodeType, key: &str, val: f64) -> bool {
        (&node.node_type == nt
            && matches!(node.attributes.get(key), Some(Scalar::Number(n)) if (n - val).abs() < 1e-9))
            || node
                .children
                .iter()
                .any(|c| has_attr_number(c, nt, key, val))
    }

    #[test]
    fn header_align_sets_column_alignment() {
        // `{% align %}` on header cells becomes the table's column `align`.
        let src = "{% table %}\n* L\n* C {% align=\"center\" %}\n* R {% align=\"right\" %}\n---\n* a\n* b\n* c\n{% /table %}\n";
        let doc = crate::parse(src, None).unwrap();
        let table = find_node(&doc, &NodeType::Table).expect("table node");
        match table.attributes.get("align") {
            Some(Scalar::Array(a)) => {
                let names: Vec<&str> = a
                    .iter()
                    .filter_map(|s| match s {
                        Scalar::String(x) => Some(x.as_str()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(names, vec!["", "center", "right"]);
            }
            other => panic!("expected an align array, got {other:?}"),
        }
    }

    #[test]
    fn colspan_annotation_stored_on_cell() {
        // `{% colspan=2 %}` on a body cell is stored as a numeric attribute,
        // and does not swallow the following cells.
        let src = "{% table %}\n---\n* a {% colspan=2 %}\n* b\n{% /table %}\n";
        let doc = crate::parse(src, None).unwrap();
        let table = find_node(&doc, &NodeType::Table).expect("table node");
        assert!(
            has_attr_number(table, &NodeType::Td, "colspan", 2.0),
            "a td carries colspan=2, tree was {table:#?}"
        );
        assert_eq!(
            count_nodes(table, &NodeType::Td),
            2,
            "both cells survive (annotation didn't swallow the next cell)"
        );
    }

    #[test]
    fn if_inside_list_table_is_preserved_until_evaluate() {
        let src = r#"{% table %}
* Task
* Pass
---
* Always
*
{% if equals($model, $shelf) %}
---
* Shelf only
*
{% /if %}
{% /table %}
"#;
        let doc = crate::parse(src, None).unwrap();
        let table = find_node(&doc, &NodeType::Table).expect("table");
        assert!(
            has_tag(table, "if"),
            "if wrapping conditional rows must survive parse, tree was {table:#?}"
        );
        assert!(count_text(table, "Shelf only") > 0);
        assert!(count_text(table, "Always") > 0);
    }

    #[test]
    fn falsy_if_inside_list_table_drops_rows() {
        let src = r#"{% table %}
* Task
* Pass
---
* Always
*
{% if equals($model, $shelf) %}
---
* Shelf only
*
{% /if %}
* After
*
{% /table %}
"#;
        let doc = crate::parse(src, None).unwrap();
        let ctx = ctx_with(&[
            ("model", Scalar::String("mir250".into())),
            ("shelf", Scalar::String("mir250_shelf_carrier".into())),
        ]);
        let result = evaluate_conditionals(&doc, &ctx).unwrap();
        let table = find_node(&result, &NodeType::Table).expect("table");
        assert!(!has_tag(table, "if"), "if must be resolved");
        assert_eq!(count_text(table, "Shelf only"), 0);
        assert!(count_text(table, "Always") > 0);
        assert!(count_text(table, "After") > 0);
        assert_eq!(count_nodes(table, &NodeType::Tr), 3); // header + 2 body
    }

    #[test]
    fn truthy_if_inside_list_table_keeps_rows() {
        let src = r#"{% table %}
* Task
* Pass
---
* Always
*
{% if equals($model, $shelf) %}
---
* Shelf only
*
{% /if %}
{% /table %}
"#;
        let doc = crate::parse(src, None).unwrap();
        let ctx = ctx_with(&[
            ("model", Scalar::String("mir250_shelf_carrier".into())),
            ("shelf", Scalar::String("mir250_shelf_carrier".into())),
        ]);
        let result = evaluate_conditionals(&doc, &ctx).unwrap();
        let table = find_node(&result, &NodeType::Table).expect("table");
        assert!(!has_tag(table, "if"));
        assert!(count_text(table, "Shelf only") > 0);
        assert_eq!(count_nodes(table, &NodeType::Tr), 3); // header + Always + Shelf
    }

    #[test]
    fn if_rows_without_leading_sep_and_following_row() {
        // periodic_tasks Component replacements pattern: if body starts with
        // cells (no --- after {% if %}), then unconditional rows follow the
        // closing {% /if %} without a --- between them.
        let src = r#"{% table %}
* Component
* Years
---
{% if equals($model, $shelf) %}
* Shelf encoder
* 6
---
* Shelf harness
* 13
{% /if %}
* Emergency stop
* 20
{% /table %}
"#;
        let doc = crate::parse(src, None).unwrap();
        let ctx = ctx_with(&[
            ("model", Scalar::String("mir250".into())),
            ("shelf", Scalar::String("mir250_shelf_carrier".into())),
        ]);
        let result = evaluate_conditionals(&doc, &ctx).unwrap();
        let table = find_node(&result, &NodeType::Table).expect("table");
        assert_eq!(count_text(table, "Shelf encoder"), 0);
        assert_eq!(count_text(table, "Shelf harness"), 0);
        assert!(count_text(table, "Emergency stop") > 0);
        assert_eq!(count_nodes(table, &NodeType::Tr), 2); // header + Emergency
    }

    #[test]
    fn list_table_preserves_column_weights_attr() {
        let src = r#"{% table column_weights="1 2.5" %}
* H1
* H2
---
* a
* b
{% /table %}
"#;
        let doc = crate::parse(src, None).unwrap();
        let table = find_node(&doc, &NodeType::Table).expect("table node");
        assert_eq!(
            table.attributes.get("column_weights"),
            Some(&Scalar::String("1 2.5".into())),
            "opening-tag column_weights must land on the Table node, tree was {table:#?}"
        );
        assert_eq!(count_nodes(table, &NodeType::Td), 2);
    }
}
