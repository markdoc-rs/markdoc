//! Cross-reference resolution for `{% tag %}` / `{% tagref %}`.
//!
//! Two-pass walk over a (post-partial-expansion) `Node` tree:
//!
//!   1. **collect**: every `{% tag id="x" %}` declaration's id is recorded
//!      in an anchor table.
//!   2. **annotate**: every `{% tagref id="x" %}` is checked against the
//!      table; broken refs get a [`ValidationError`] attached to the ref's
//!      `errors` field (the Node otherwise survives so renderers can still
//!      show "[broken ref: x]" or similar).
//!
//! Both forms accept the id under `id=`, `tag=`, or the Markdoc primary
//! shorthand (`{% tag "x" /%}` / `{% tagref "x" /%}`). The Flux spec
//! README uses `id=` while real docs often use the primary form.
//!
//! Note on naming: `{% tag %}` is the user-facing Flux tag name for an
//! anchor declaration. It is unrelated to the internal `NodeType::Tag`
//! that wraps any custom Markdoc tag — they only share a noun.
//!
//! Run between partial expansion and transform:
//! ```ignore
//! let doc = markdoc::parse(&src, None)?;
//! let doc = markdoc::expand_partials(&doc, &resolver)?;
//! let doc = markdoc::resolve_crossrefs(&doc);
//! let rendered = markdoc::transform(&doc, &Config::default())?;
//! ```

use crate::ast::Node;
use crate::types::*;
use std::collections::HashMap;

/// Resolve cross-references in a Node tree.
///
/// Always succeeds — broken references surface as `ValidationError`s
/// attached to the offending `tagref` nodes' `errors` fields, not as
/// a Result::Err. This keeps renderers able to produce output even
/// when some refs are broken.
pub fn resolve_crossrefs(node: &Node) -> Node {
    let anchors = collect_anchors(node);
    annotate_refs(node, &anchors)
}

#[derive(Debug, Clone)]
pub struct AnchorInfo {
    pub id: String,
    /// Lines of the declaring node (best-effort, currently always 1
    /// because the parser stubs line tracking).
    pub lines: Vec<usize>,
}

/// Walk the tree collecting every declared `{% tag %}` anchor.
///
/// On duplicate ids the later declaration wins. We could surface a
/// warning here in the future; for now duplicates silently overwrite.
pub fn collect_anchors(node: &Node) -> HashMap<String, AnchorInfo> {
    let mut anchors = HashMap::new();
    walk_collect(node, &mut anchors);
    anchors
}

fn walk_collect(node: &Node, anchors: &mut HashMap<String, AnchorInfo>) {
    if is_tag_node(node, "tag")
        && let Some(id) = anchor_id(node)
    {
        anchors.insert(
            id.clone(),
            AnchorInfo {
                id,
                lines: node.lines.clone(),
            },
        );
    }
    for child in &node.children {
        walk_collect(child, anchors);
    }
}

fn annotate_refs(node: &Node, anchors: &HashMap<String, AnchorInfo>) -> Node {
    let mut new_node = node.clone();
    new_node.children = node
        .children
        .iter()
        .map(|c| annotate_refs(c, anchors))
        .collect();

    if is_tag_node(&new_node, "tagref") {
        match anchor_id(&new_node) {
            None => {
                new_node.errors.push(ValidationError {
                    id: "tagref/missing-id".to_string(),
                    level: ValidationLevel::Error,
                    message: "{% tagref %} requires an `id`, `tag`, or primary attribute".into(),
                    location: new_node.location.clone(),
                });
            }
            Some(id) if !anchors.contains_key(&id) => {
                new_node.errors.push(ValidationError {
                    id: "tagref/unknown-target".to_string(),
                    level: ValidationLevel::Error,
                    message: format!("Unknown cross-reference target: {id:?}"),
                    location: new_node.location.clone(),
                });
            }
            _ => {}
        }
    }

    new_node
}

fn is_tag_node(node: &Node, tag_name: &str) -> bool {
    matches!(node.node_type, NodeType::Tag) && node.tag.as_deref() == Some(tag_name)
}

/// Return the anchor id from `id`, `tag`, or Markdoc primary shorthand
/// (`{% tag "x" /%}` / `{% tagref "x" /%}`), in that order.
fn anchor_id(node: &Node) -> Option<String> {
    for key in ["id", "tag", "primary"] {
        if let Some(Scalar::String(s)) = node.attributes.get(key)
            && !s.is_empty()
        {
            return Some(s.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn find_first_tag<'a>(node: &'a Node, tag_name: &str) -> Option<&'a Node> {
        if matches!(node.node_type, NodeType::Tag) && node.tag.as_deref() == Some(tag_name) {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|c| find_first_tag(c, tag_name))
    }

    fn count_tags(node: &Node, tag_name: &str) -> usize {
        let mut n = 0;
        if matches!(node.node_type, NodeType::Tag) && node.tag.as_deref() == Some(tag_name) {
            n += 1;
        }
        for c in &node.children {
            n += count_tags(c, tag_name);
        }
        n
    }

    #[test]
    fn matched_ref_has_no_error() {
        let src = r#"# Title {% tag id="x" /%}

See {% tagref id="x" /%}"#;
        let doc = parse(src, None).unwrap();
        let resolved = resolve_crossrefs(&doc);
        let tagref = find_first_tag(&resolved, "tagref").expect("tagref present");
        assert!(
            tagref.errors.is_empty(),
            "expected no errors, got {:?}",
            tagref.errors
        );
    }

    #[test]
    fn unmatched_ref_emits_error() {
        let src = r#"See {% tagref id="missing" /%}"#;
        let doc = parse(src, None).unwrap();
        let resolved = resolve_crossrefs(&doc);
        let tagref = find_first_tag(&resolved, "tagref").unwrap();
        assert_eq!(tagref.errors.len(), 1);
        assert_eq!(tagref.errors[0].id, "tagref/unknown-target");
        assert!(tagref.errors[0].message.contains("missing"));
    }

    #[test]
    fn collect_anchors_finds_all_declarations() {
        let src = r#"
{% tag id="alpha" /%}
{% tag id="beta" /%}
{% tag tag="gamma" /%}
"#;
        let doc = parse(src, None).unwrap();
        let anchors = collect_anchors(&doc);
        assert!(anchors.contains_key("alpha"));
        assert!(anchors.contains_key("beta"));
        assert!(anchors.contains_key("gamma"));
        assert_eq!(anchors.len(), 3);
    }

    #[test]
    fn supports_id_then_tag_attribute_for_tagref() {
        // tag declares with `id=`, tagref references with `tag=`.
        let src = r#"{% tag id="x" /%} ref: {% tagref tag="x" /%}"#;
        let doc = parse(src, None).unwrap();
        let resolved = resolve_crossrefs(&doc);
        let tagref = find_first_tag(&resolved, "tagref").unwrap();
        assert!(tagref.errors.is_empty(), "should resolve via `tag=`");
    }

    #[test]
    fn supports_primary_shorthand_for_tag_and_tagref() {
        // Content manuals use `{% tag "x" /%}` / `{% tagref "x" /%}`.
        let src = r#"# Title {% tag "x" /%}

See {% tagref "x" /%}"#;
        let doc = parse(src, None).unwrap();
        let resolved = resolve_crossrefs(&doc);
        let tagref = find_first_tag(&resolved, "tagref").unwrap();
        assert!(
            tagref.errors.is_empty(),
            "should resolve via primary shorthand, got {:?}",
            tagref.errors
        );
        let anchors = collect_anchors(&doc);
        assert!(anchors.contains_key("x"));
    }

    #[test]
    fn supports_tag_then_id_attribute_for_tagref() {
        // tag declares with `tag=`, tagref references with `id=`.
        let src = r#"{% tag tag="x" /%} ref: {% tagref id="x" /%}"#;
        let doc = parse(src, None).unwrap();
        let resolved = resolve_crossrefs(&doc);
        let tagref = find_first_tag(&resolved, "tagref").unwrap();
        assert!(tagref.errors.is_empty(), "should resolve via `tag=`");
    }

    #[test]
    fn missing_id_attr_on_tagref_is_error() {
        let src = "{% tagref /%}";
        let doc = parse(src, None).unwrap();
        let resolved = resolve_crossrefs(&doc);
        let tagref = find_first_tag(&resolved, "tagref").unwrap();
        assert_eq!(tagref.errors.len(), 1);
        assert_eq!(tagref.errors[0].id, "tagref/missing-id");
    }

    #[test]
    fn multiple_refs_to_same_anchor_all_resolve() {
        let src = r#"
{% tag id="x" /%}

A: {% tagref id="x" /%}, B: {% tagref id="x" /%}, C: {% tagref id="x" /%}
"#;
        let doc = parse(src, None).unwrap();
        let resolved = resolve_crossrefs(&doc);
        assert_eq!(count_tags(&resolved, "tagref"), 3);
        // None of the three refs should have errors.
        fn assert_clean(n: &Node) {
            if matches!(n.node_type, NodeType::Tag) && n.tag.as_deref() == Some("tagref") {
                assert!(n.errors.is_empty());
            }
            for c in &n.children {
                assert_clean(c);
            }
        }
        assert_clean(&resolved);
    }

    #[test]
    fn tagref_unaffected_when_no_anchor_declared() {
        // No declarations anywhere; both refs broken.
        let src = r#"{% tagref id="a" /%} and {% tagref id="b" /%}"#;
        let doc = parse(src, None).unwrap();
        let resolved = resolve_crossrefs(&doc);
        let mut errors = 0;
        fn count_errors(n: &Node, c: &mut usize) {
            *c += n.errors.len();
            for child in &n.children {
                count_errors(child, c);
            }
        }
        count_errors(&resolved, &mut errors);
        assert_eq!(errors, 2);
    }

    #[test]
    fn block_form_tag_with_body_still_collected() {
        // `{% tag id="x" %}body{% /tag %}` rather than self-closing.
        let src = r#"{% tag id="block-form" %}some content{% /tag %}

ref: {% tagref id="block-form" /%}"#;
        let doc = parse(src, None).unwrap();
        let resolved = resolve_crossrefs(&doc);
        let tagref = find_first_tag(&resolved, "tagref").unwrap();
        assert!(tagref.errors.is_empty());
    }
}
