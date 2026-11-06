//! Conditional resolution for `{% if %}` tags.
//!
//! Walks a parsed Node tree and, for each `{% if %}` tag, evaluates its
//! primary expression against the supplied Context. If truthy, splices
//! the if's children at the if's position. If falsy, drops the subtree.
//!
//! Run between cross-reference resolution and transform:
//! ```ignore
//! let doc = markdoc::parse(&src, None)?;
//! let doc = markdoc::expand_partials(&doc, &resolver)?;
//! let doc = markdoc::resolve_crossrefs(&doc);
//! let doc = markdoc::evaluate_conditionals(&doc, &ctx)?;
//! let rendered = markdoc::transform_with_context(&doc, &Config::default(), &ctx)?;
//! ```
//!
//! Predicates accept anything the expression module accepts:
//! `$config.foo`, `equals($a, "x")`, `and($a, not($b))`, plus literal
//! `true` / `false` / numbers / strings (via the schema).
//!
//! Truthiness uses `tags::truthy`: `null`/`false`/`0`/`""`/`[]`/`{}` are
//! all falsy; everything else is truthy.

use crate::ast::Node;
use crate::expression::{default_function_impls, evaluate, parse_expression};
use crate::functions::FunctionImpl;
use crate::tags::truthy;
use crate::types::*;
use std::collections::HashMap;

/// Resolve every `{% if %}` tag in `node` against `ctx`. Returns a new
/// tree with conditionals resolved (splice / drop).
pub fn evaluate_conditionals(node: &Node, ctx: &Context) -> Result<Node> {
    evaluate_conditionals_with_functions(node, ctx, default_function_impls())
}

/// As `evaluate_conditionals`, with caller-supplied functions.
///
/// `{% if %}` predicates may call functions, and before this the table was
/// fixed at `expression.rs`'s built-ins — so a document using a function this
/// crate does not define failed to render and there was no way to supply one
/// from outside. Build `fns` with `expression::functions_with`.
pub fn evaluate_conditionals_with_functions(
    node: &Node,
    ctx: &Context,
    fns: &HashMap<String, FunctionImpl>,
) -> Result<Node> {
    let nodes = walk(node, ctx, fns)?;
    if nodes.len() == 1 {
        Ok(nodes.into_iter().next().unwrap())
    } else if nodes.is_empty() {
        // Top-level node was an `if` that evaluated to falsy. Return an
        // empty Document so the caller still has a valid root.
        Ok(Node::new(
            NodeType::Document,
            HashMap::new(),
            Vec::new(),
            None,
        ))
    } else {
        // Top-level was a truthy `if` that produced multiple children;
        // wrap them in a synthetic Document.
        Ok(Node::new(NodeType::Document, HashMap::new(), nodes, None))
    }
}

fn walk(node: &Node, ctx: &Context, fns: &HashMap<String, FunctionImpl>) -> Result<Vec<Node>> {
    if is_if_tag(node) {
        return evaluate_if(node, ctx, fns);
    }
    let mut new_children = Vec::with_capacity(node.children.len());
    for child in &node.children {
        new_children.extend(walk(child, ctx, fns)?);
    }
    let mut new_node = node.clone();
    new_node.children = new_children;
    Ok(vec![new_node])
}

fn is_if_tag(node: &Node) -> bool {
    matches!(node.node_type, NodeType::Tag) && node.tag.as_deref() == Some("if")
}

fn evaluate_if(
    node: &Node,
    ctx: &Context,
    fns: &HashMap<String, FunctionImpl>,
) -> Result<Vec<Node>> {
    // Build the branch table: a Vec of (predicate_source, body_nodes).
    // The first entry's predicate is the `if`'s own; each subsequent
    // `{% else %}` self-closing tag among `node.children` opens a new
    // branch using its own predicate (or no predicate = unconditional
    // fallback).
    let mut branches: Vec<(Option<&Node>, Vec<&Node>)> = vec![(None, Vec::new())];
    // Mark the first branch as "use the if's own predicate".
    let if_branch_marker = node;
    let mut first_pred_owner: Option<&Node> = Some(if_branch_marker);
    branches[0].0 = first_pred_owner.take();

    for child in &node.children {
        if is_else_tag(child) {
            // Self-closing tag — its primary attribute (if present) is
            // the new branch's predicate.
            branches.push((Some(child), Vec::new()));
        } else {
            branches.last_mut().unwrap().1.push(child);
        }
    }

    // Evaluate branches in order; first truthy predicate wins.
    for (pred_source, body) in &branches {
        let predicate_value = match pred_source {
            // The `if`'s own branch (predicate lives on `node`).
            Some(p) if std::ptr::eq(*p, node) => predicate_for(node, ctx, fns)?,
            // An `{% else $cond /%}` branch (predicate on the else tag).
            Some(p) => predicate_for_else(p, ctx, fns)?,
            // Unreachable — every branch has a Some(_) source by construction.
            None => Scalar::Null,
        };
        if truthy(&predicate_value) {
            let mut spliced = Vec::new();
            for child in body {
                spliced.extend(walk(child, ctx, fns)?);
            }
            return Ok(spliced);
        }
    }
    // No branch matched.
    Ok(Vec::new())
}

fn is_else_tag(node: &Node) -> bool {
    matches!(node.node_type, NodeType::Tag) && node.tag.as_deref() == Some("else")
}

/// Resolve the predicate on an `{% if %}` tag — either an unresolved
/// expression or a literal scalar. Missing predicate is treated as falsy.
fn predicate_for(
    node: &Node,
    ctx: &Context,
    fns: &HashMap<String, FunctionImpl>,
) -> Result<Scalar> {
    if let Some(source) = node.expressions.get("primary") {
        let expr = parse_expression(source).map_err(|e| {
            MarkdocError::TransformError(format!("Failed to parse if predicate {source:?}: {e}"))
        })?;
        return evaluate(&expr, ctx, fns);
    }
    if let Some(literal) = node.attributes.get("primary") {
        return Ok(literal.clone());
    }
    Ok(Scalar::Null)
}

/// Resolve the predicate on an `{% else %}` tag. A bare `{% else /%}`
/// (no primary attribute, no expression) is treated as truthy — the
/// unconditional fallback. An `{% else $cond /%}` evaluates `$cond`.
fn predicate_for_else(
    node: &Node,
    ctx: &Context,
    fns: &HashMap<String, FunctionImpl>,
) -> Result<Scalar> {
    if let Some(source) = node.expressions.get("primary") {
        let expr = parse_expression(source).map_err(|e| {
            MarkdocError::TransformError(format!("Failed to parse else predicate {source:?}: {e}"))
        })?;
        return evaluate(&expr, ctx, fns);
    }
    if let Some(literal) = node.attributes.get("primary") {
        return Ok(literal.clone());
    }
    // Bare `{% else /%}` — treat as truthy fallback.
    Ok(Scalar::Boolean(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

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
    fn truthy_literal_keeps_children() {
        let src = "{% if true %}visible content{% /if %}";
        let doc = parse(src, None).unwrap();
        let result = evaluate_conditionals(&doc, &Context::new()).unwrap();
        assert!(count_text(&result, "visible content") > 0);
        assert!(!has_tag(&result, "if"));
    }

    #[test]
    fn falsy_literal_drops_subtree() {
        let src = "before {% if false %}hidden{% /if %} after";
        let doc = parse(src, None).unwrap();
        let result = evaluate_conditionals(&doc, &Context::new()).unwrap();
        assert_eq!(count_text(&result, "hidden"), 0);
        assert!(count_text(&result, "before") > 0);
        assert!(count_text(&result, "after") > 0);
        assert!(!has_tag(&result, "if"));
    }

    #[test]
    fn variable_predicate_resolves_truthy() {
        let src = "{% if $config.show %}content{% /if %}";
        let doc = parse(src, None).unwrap();
        let mut cfg = HashMap::new();
        cfg.insert("show".to_string(), Scalar::Boolean(true));
        let ctx = ctx_with(&[("config", Scalar::Object(cfg))]);
        let result = evaluate_conditionals(&doc, &ctx).unwrap();
        assert!(count_text(&result, "content") > 0);
    }

    #[test]
    fn variable_predicate_resolves_falsy() {
        let src = "{% if $config.show %}hidden{% /if %}";
        let doc = parse(src, None).unwrap();
        let mut cfg = HashMap::new();
        cfg.insert("show".to_string(), Scalar::Boolean(false));
        let ctx = ctx_with(&[("config", Scalar::Object(cfg))]);
        let result = evaluate_conditionals(&doc, &ctx).unwrap();
        assert_eq!(count_text(&result, "hidden"), 0);
    }

    #[test]
    fn missing_variable_treated_as_falsy() {
        // `$config.absent` resolves to Null in an empty context → falsy.
        let src = "{% if $config.absent %}should not appear{% /if %}";
        let doc = parse(src, None).unwrap();
        let result = evaluate_conditionals(&doc, &Context::new()).unwrap();
        assert_eq!(count_text(&result, "should not appear"), 0);
    }

    #[test]
    fn function_call_predicate_evaluates() {
        // and(equals($a, $b), not($c))  with a=b="x", c=false  → true
        let src = r#"{% if and(equals($a, $b), not($c)) %}kept{% /if %}"#;
        let doc = parse(src, None).unwrap();
        let mut top = HashMap::new();
        top.insert("a".to_string(), Scalar::String("x".to_string()));
        top.insert("b".to_string(), Scalar::String("x".to_string()));
        top.insert("c".to_string(), Scalar::Boolean(false));
        let ctx = Context { variables: top };
        let result = evaluate_conditionals(&doc, &ctx).unwrap();
        assert!(count_text(&result, "kept") > 0);
    }

    #[test]
    fn nested_ifs_resolve_inside_out() {
        // Outer truthy + inner falsy → outer keeps its non-inner content,
        // inner is dropped.
        let src = r#"{% if true %}outer kept{% if false %}inner dropped{% /if %}{% /if %}"#;
        let doc = parse(src, None).unwrap();
        let result = evaluate_conditionals(&doc, &Context::new()).unwrap();
        assert!(count_text(&result, "outer kept") > 0);
        assert_eq!(count_text(&result, "inner dropped"), 0);
        assert!(!has_tag(&result, "if"));
    }

    #[test]
    fn if_inside_callout_resolves_correctly() {
        // The safety_functions doc pattern: per-feature `if` inside a
        // structural tag. The structural tag must survive; the `if` must
        // resolve to its children (or nothing).
        let src = r#"{% callout type="warning" %}
{% if $config.has_estop %}E-stop is present.{% /if %}
{% /callout %}"#;
        let doc = parse(src, None).unwrap();
        let mut cfg = HashMap::new();
        cfg.insert("has_estop".to_string(), Scalar::Boolean(true));
        let ctx = ctx_with(&[("config", Scalar::Object(cfg))]);
        let result = evaluate_conditionals(&doc, &ctx).unwrap();
        assert!(has_tag(&result, "callout"));
        assert!(!has_tag(&result, "if"));
        assert!(count_text(&result, "E-stop is present.") > 0);
    }

    #[test]
    fn if_with_no_predicate_is_falsy() {
        // `{% if %}body{% /if %}` — no predicate → drop.
        let src = "{% if %}body{% /if %}";
        let doc = parse(src, None).unwrap();
        let result = evaluate_conditionals(&doc, &Context::new()).unwrap();
        assert_eq!(count_text(&result, "body"), 0);
    }

    #[test]
    fn empty_string_predicate_is_falsy() {
        let src = r#"{% if $config.name %}greet{% /if %}"#;
        let doc = parse(src, None).unwrap();
        let mut cfg = HashMap::new();
        cfg.insert("name".to_string(), Scalar::String(String::new())); // empty
        let ctx = ctx_with(&[("config", Scalar::Object(cfg))]);
        let result = evaluate_conditionals(&doc, &ctx).unwrap();
        assert_eq!(count_text(&result, "greet"), 0);
    }

    #[test]
    fn non_empty_string_predicate_is_truthy() {
        let src = r#"{% if $config.name %}greet{% /if %}"#;
        let doc = parse(src, None).unwrap();
        let mut cfg = HashMap::new();
        cfg.insert("name".to_string(), Scalar::String("Alice".into()));
        let ctx = ctx_with(&[("config", Scalar::Object(cfg))]);
        let result = evaluate_conditionals(&doc, &ctx).unwrap();
        assert!(count_text(&result, "greet") > 0);
    }

    // ── else / else-if branches ─────────────────────────────────────────

    #[test]
    fn else_branch_taken_when_if_falsy() {
        let src = "{% if false %}A{% else /%}B{% /if %}";
        let doc = parse(src, None).unwrap();
        let result = evaluate_conditionals(&doc, &Context::new()).unwrap();
        assert_eq!(count_text(&result, "A"), 0);
        assert!(count_text(&result, "B") > 0);
    }

    #[test]
    fn else_branch_skipped_when_if_truthy() {
        let src = "{% if true %}A{% else /%}B{% /if %}";
        let doc = parse(src, None).unwrap();
        let result = evaluate_conditionals(&doc, &Context::new()).unwrap();
        assert!(count_text(&result, "A") > 0);
        assert_eq!(count_text(&result, "B"), 0);
    }

    #[test]
    fn else_if_picks_first_truthy_branch() {
        let src = "{% if false %}A{% else $b /%}B{% else $c /%}C{% else /%}D{% /if %}";
        let doc = parse(src, None).unwrap();
        let ctx = ctx_with(&[("b", Scalar::Boolean(false)), ("c", Scalar::Boolean(true))]);
        let result = evaluate_conditionals(&doc, &ctx).unwrap();
        assert_eq!(count_text(&result, "A"), 0);
        assert_eq!(count_text(&result, "B"), 0);
        assert!(count_text(&result, "C") > 0);
        assert_eq!(count_text(&result, "D"), 0);
    }

    #[test]
    fn else_if_chain_falls_through_to_unconditional_fallback() {
        let src = "{% if false %}A{% else $b /%}B{% else /%}C{% /if %}";
        let doc = parse(src, None).unwrap();
        let ctx = ctx_with(&[("b", Scalar::Boolean(false))]);
        let result = evaluate_conditionals(&doc, &ctx).unwrap();
        assert_eq!(count_text(&result, "A"), 0);
        assert_eq!(count_text(&result, "B"), 0);
        assert!(count_text(&result, "C") > 0);
    }

    #[test]
    fn no_branch_matches_drops_subtree() {
        let src = "before {% if false %}A{% else $b /%}B{% /if %} after";
        let doc = parse(src, None).unwrap();
        // No fallback `{% else /%}`, predicate $b absent → drops both.
        let result = evaluate_conditionals(&doc, &Context::new()).unwrap();
        assert_eq!(count_text(&result, "A"), 0);
        assert_eq!(count_text(&result, "B"), 0);
        assert!(count_text(&result, "before") > 0);
        assert!(count_text(&result, "after") > 0);
    }
}
