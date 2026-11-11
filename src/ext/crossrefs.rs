//! Cross-references between `{% tag %}` anchors and `{% tagref %}` references (an extension,
//! off unless called).
//!
//! An AST pass over a parsed document (after partials are expanded, before `transform`):
//!
//! 1. **collect**: every `{% tag %}` declaration's id goes into an anchor table
//!    ([`collect_anchors`]);
//! 2. **annotate**: every `{% tagref %}` is checked against the table; a broken one gets a
//!    [`ValidationError`] in its `errors` (`tagref/missing-id` without an id,
//!    `tagref/unknown-target` for an id no anchor declares). The node stays, so a renderer can
//!    still show it.
//!
//! Both tags take the id as `id=`, `tag=` or the primary value (`{% tag "x" /%}`), in that
//! order. `{% tag %}` is the anchor tag of the documents this crate renders; it is
//! unrelated to [`NodeType::Tag`], the type of every tag node.
//!
//! Upstream Markdoc has no such pass: nothing here runs unless called, and
//! [`resolve_crossrefs`] changes nothing but `tagref` nodes' `errors`.
//!
//! ```
//! use markdoc::ext::crossrefs::{collect_anchors, resolve_crossrefs};
//! use markdoc::next::{index, parser::ParserArgs};
//!
//! let doc = index::parse(
//!     "# Title {% tag \"intro\" /%}\n\nSee {% tagref \"intro\" /%} and {% tagref \"gone\" /%}.\n",
//!     &ParserArgs::default(),
//! )
//! .unwrap();
//! assert!(collect_anchors(&doc).contains_key("intro"));
//! let checked = resolve_crossrefs(&doc);
//! let errors: Vec<&str> = checked
//!     .walk()
//!     .flat_map(|node| node.errors.iter().map(|e| e.id.as_str()))
//!     .collect();
//! assert_eq!(errors, ["tagref/unknown-target"]);
//! ```

use crate::ast::Node;
use crate::types::{NodeType, Scalar, ValidationError, ValidationLevel};
use indexmap::IndexMap;

/// `node` with every `{% tagref %}` checked against the `{% tag %}` anchors of the same tree.
///
/// Never fails: a broken reference is a [`ValidationError`] on the `tagref` node's `errors`
/// (`tagref/missing-id`, `tagref/unknown-target`), not an `Err`, so that a renderer can still
/// render the document. Children and slots are both searched. Any depth (no recursion).
pub fn resolve_crossrefs(node: &Node) -> Node {
    let anchors = collect_anchors(node);
    let mut out = node.clone();
    let mut stack: Vec<&mut Node> = vec![&mut out];
    while let Some(node) = stack.pop() {
        if is_tag(node, "tagref") {
            check_ref(node, &anchors);
        }
        let Node {
            children, slots, ..
        } = node;
        stack.extend(slots.values_mut());
        stack.extend(children.iter_mut());
    }
    out
}

/// A `{% tag %}` anchor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorInfo {
    pub id: String,
    /// The declaring node's `lines`, as the parser gave them: upstream's, 0-based (the
    /// node's [`span`](crate::ast::Node::span) has this crate's 1-based position).
    pub lines: Vec<usize>,
}

/// Every `{% tag %}` anchor of the tree (children and slots), by id, in document order. A
/// later declaration of the same id replaces the earlier one's entry (where the first one
/// was).
pub fn collect_anchors(node: &Node) -> IndexMap<String, AnchorInfo> {
    let mut anchors = IndexMap::new();
    for node in std::iter::once(node).chain(node.walk()) {
        if is_tag(node, "tag")
            && let Some(id) = anchor_id(node)
        {
            anchors.insert(
                id.clone(),
                AnchorInfo {
                    id,
                    lines: node.lines.to_vec(),
                },
            );
        }
    }
    anchors
}

/// The error a broken `tagref` gets, placed where the node is: this crate's position of the
/// node in `span` (`None` unless the document was parsed with spans on,
/// [`crate::parser::ParserArgs::spans`]); upstream's `location` stays unset, as on every
/// error Markdoc makes itself, and `validate()` places it by the node's `lines`.
fn error(node: &Node, id: &str, message: String) -> ValidationError {
    ValidationError {
        span: node.span.clone(),
        ..ValidationError::new(id, ValidationLevel::Error, message)
    }
}

fn check_ref(node: &mut Node, anchors: &IndexMap<String, AnchorInfo>) {
    let error = match anchor_id(node) {
        None => error(
            node,
            "tagref/missing-id",
            "{% tagref %} requires an `id`, `tag`, or primary attribute".into(),
        ),
        Some(id) if !anchors.contains_key(&id) => error(
            node,
            "tagref/unknown-target",
            format!("Unknown cross-reference target: {id:?}"),
        ),
        Some(_) => return,
    };
    node.errors.push(error);
}

fn is_tag(node: &Node, name: &str) -> bool {
    node.node_type == NodeType::Tag && node.tag.as_deref() == Some(name)
}

/// The anchor id: the first non-empty string of `id`, `tag` and `primary`.
fn anchor_id(node: &Node) -> Option<String> {
    ["id", "tag", "primary"]
        .iter()
        .find_map(|key| match node.attributes.get(*key) {
            Some(Scalar::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    //! The old pipeline's tests (`crate::crossrefs`), on `next`'s AST.
    use super::*;
    use crate::next::index::{parse, validate};
    use crate::next::parser::ParserArgs;
    use crate::next::schema::{Config, Schema};

    fn doc(src: &str) -> Node {
        parse(src, &ParserArgs::default()).unwrap()
    }

    fn find_first_tag<'a>(node: &'a Node, name: &str) -> Option<&'a Node> {
        node.walk().find(|n| is_tag(n, name))
    }

    fn count_tags(node: &Node, name: &str) -> usize {
        node.walk().filter(|n| is_tag(n, name)).count()
    }

    fn all_errors(node: &Node) -> Vec<String> {
        std::iter::once(node)
            .chain(node.walk())
            .flat_map(|n| n.errors.iter().map(|e| e.id.clone()))
            .collect()
    }

    #[test]
    fn matched_ref_has_no_error() {
        let resolved = resolve_crossrefs(&doc(
            "# Title {% tag id=\"x\" /%}\n\nSee {% tagref id=\"x\" /%}",
        ));
        let tagref = find_first_tag(&resolved, "tagref").expect("tagref present");
        assert!(tagref.errors.is_empty(), "{:?}", tagref.errors);
    }

    #[test]
    fn unmatched_ref_emits_error() {
        let src = r#"See {% tagref id="missing" /%}"#;
        let spans = ParserArgs {
            spans: true,
            ..ParserArgs::default()
        };
        let resolved = resolve_crossrefs(&parse(src, &spans).unwrap());
        let tagref = find_first_tag(&resolved, "tagref").unwrap();
        assert_eq!(tagref.errors.len(), 1);
        assert_eq!(tagref.errors[0].id, "tagref/unknown-target");
        assert!(tagref.errors[0].message.contains("missing"));
        // Placed where the reference is (this crate's span, with spans on; upstream's
        // location is unset).
        assert!(tagref.errors[0].location.is_none());
        let span = tagref.errors[0].span.as_ref().unwrap();
        assert_eq!(span.start.offset, 4);
        // Spans off (the default), the error has no position of its own; `validate()` still
        // places it by the node's `lines` and `location`.
        let resolved = resolve_crossrefs(&doc(src));
        let tagref = find_first_tag(&resolved, "tagref").unwrap();
        assert!(tagref.errors[0].span.is_none() && tagref.errors[0].location.is_none());
        assert_eq!(tagref.lines, [0, 1]);
    }

    #[test]
    fn collect_anchors_finds_all_declarations() {
        let anchors = collect_anchors(&doc(
            "\n{% tag id=\"alpha\" /%}\n{% tag id=\"beta\" /%}\n{% tag tag=\"gamma\" /%}\n",
        ));
        assert_eq!(
            anchors.keys().collect::<Vec<_>>(),
            ["alpha", "beta", "gamma"]
        );
        // Upstream's lines: 0-based, from the token's map (the tag is on the third line).
        assert_eq!(anchors["beta"].lines, [2, 3]);
    }

    #[test]
    fn supports_id_then_tag_attribute_for_tagref() {
        let resolved = resolve_crossrefs(&doc(r#"{% tag id="x" /%} ref: {% tagref tag="x" /%}"#));
        assert!(
            find_first_tag(&resolved, "tagref")
                .unwrap()
                .errors
                .is_empty()
        );
    }

    #[test]
    fn supports_primary_shorthand_for_tag_and_tagref() {
        let parsed = doc("# Title {% tag \"x\" /%}\n\nSee {% tagref \"x\" /%}");
        let resolved = resolve_crossrefs(&parsed);
        assert!(
            find_first_tag(&resolved, "tagref")
                .unwrap()
                .errors
                .is_empty()
        );
        assert!(collect_anchors(&parsed).contains_key("x"));
    }

    #[test]
    fn supports_tag_then_id_attribute_for_tagref() {
        let resolved = resolve_crossrefs(&doc(r#"{% tag tag="x" /%} ref: {% tagref id="x" /%}"#));
        assert!(
            find_first_tag(&resolved, "tagref")
                .unwrap()
                .errors
                .is_empty()
        );
    }

    #[test]
    fn missing_id_attr_on_tagref_is_error() {
        let resolved = resolve_crossrefs(&doc("{% tagref /%}"));
        let tagref = find_first_tag(&resolved, "tagref").unwrap();
        assert_eq!(tagref.errors.len(), 1);
        assert_eq!(tagref.errors[0].id, "tagref/missing-id");
    }

    #[test]
    fn multiple_refs_to_same_anchor_all_resolve() {
        let resolved = resolve_crossrefs(&doc(
            "\n{% tag id=\"x\" /%}\n\nA: {% tagref id=\"x\" /%}, B: {% tagref id=\"x\" /%}, C: {% tagref id=\"x\" /%}\n",
        ));
        assert_eq!(count_tags(&resolved, "tagref"), 3);
        assert!(all_errors(&resolved).is_empty());
    }

    #[test]
    fn tagref_unaffected_when_no_anchor_declared() {
        let resolved = resolve_crossrefs(&doc(r#"{% tagref id="a" /%} and {% tagref id="b" /%}"#));
        assert_eq!(
            all_errors(&resolved),
            ["tagref/unknown-target", "tagref/unknown-target"]
        );
    }

    #[test]
    fn block_form_tag_with_body_still_collected() {
        let src = "{% tag id=\"block-form\" %}some content{% /tag %}\n\nref: {% tagref id=\"block-form\" /%}";
        let parsed = doc(src);
        let resolved = resolve_crossrefs(&parsed);
        assert!(
            find_first_tag(&resolved, "tagref")
                .unwrap()
                .errors
                .is_empty()
        );
        // Upstream's validator flags the block form when `tag` is declared self-closing (as
        // the crate's bundle declares it); this pass does not mind.
        let mut config = Config::default();
        for name in ["tag", "tagref"] {
            let schema = Schema {
                render: Some(name.into()),
                self_closing: true,
                ..Schema::default()
            };
            config.tags.insert(name.into(), Some(schema));
        }
        let ids: Vec<String> = validate(&parsed, &config)
            .unwrap()
            .into_iter()
            .map(|e| e.error.id)
            .collect();
        assert_eq!(ids, ["tag-selfclosing-has-children"]);
    }

    #[test]
    fn anchors_and_refs_in_slots_count() {
        let args = ParserArgs {
            slots: true,
            ..ParserArgs::default()
        };
        let src = "{% box %}\n{% slot \"title\" %}\nT {% tag \"s\" /%}\n{% /slot %}\nsee {% tagref \"s\" /%} {% tagref \"t\" /%}\n{% /box %}\n";
        let parsed = parse(src, &args).unwrap();
        assert!(collect_anchors(&parsed).contains_key("s"));
        assert_eq!(
            all_errors(&resolve_crossrefs(&parsed)),
            ["tagref/unknown-target"]
        );
    }

    #[test]
    fn nothing_but_tagref_errors_changes() {
        let parsed = doc("# A {% tag \"a\" /%}\n\n{% tagref \"b\" /%} {% x %}open\n");
        let resolved = resolve_crossrefs(&parsed);
        let strip = |n: &Node| {
            let mut n = n.clone();
            let mut stack = vec![&mut n];
            while let Some(node) = stack.pop() {
                if is_tag(node, "tagref") {
                    node.errors.clear();
                }
                stack.extend(node.children.iter_mut());
            }
            format!("{n:?}")
        };
        assert_eq!(strip(&parsed), strip(&resolved));
        assert_ne!(format!("{parsed:?}"), format!("{resolved:?}"));
    }

    #[test]
    fn a_deep_tree_is_no_problem() {
        // Inline tags left open nest two levels each, to the parser's depth limit.
        let src = "p {% x %} q\n".repeat(2000) + "{% tagref \"none\" /%}";
        let parsed = doc(&src);
        let handle = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || {
                let resolved = resolve_crossrefs(&parsed);
                all_errors(&resolved)
                    .iter()
                    .filter(|id| id.starts_with("tagref/"))
                    .count()
            })
            .unwrap();
        assert_eq!(handle.join().unwrap(), 1);
    }
}
