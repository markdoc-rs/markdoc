use crate::ast::Node;
use crate::expression::{default_function_impls, evaluate, parse_expression};
use crate::functions::FunctionImpl;
use crate::parser::INTERPOLATION_TAG;
use crate::types::*;
use std::collections::HashMap;

/// Transform a Node tree to a RenderableTreeNode using only the static
/// `Config`. A default `Context` is built from the document's frontmatter
/// so that `$markdoc.frontmatter.*` expressions resolve. For richer
/// resolution (e.g. `$config.*`, `$global.*`), use `transform_with_context`.
pub fn transform(node: &Node, config: &Config) -> Result<RenderableTreeNode> {
    let ctx = default_context_from(node);
    transform_with_context(node, config, &ctx)
}

/// Transform with a caller-supplied evaluation context. The context's
/// variable namespaces are used to resolve any `Node.expressions`
/// captured during parsing.
pub fn transform_with_context(
    node: &Node,
    config: &Config,
    ctx: &Context,
) -> Result<RenderableTreeNode> {
    transform_node(node, config, ctx, default_function_impls())
}

/// As `transform_with_context`, with caller-supplied functions.
///
/// Attribute expressions and `{% $var %}` interpolation evaluate here, so a
/// document whose attributes call a caller-defined function needs the same
/// table the conditionals got. Build it with `expression::functions_with`.
pub fn transform_with_functions(
    node: &Node,
    config: &Config,
    ctx: &Context,
    fns: &HashMap<String, FunctionImpl>,
) -> Result<RenderableTreeNode> {
    transform_node(node, config, ctx, fns)
}

/// Build a Context from a Document node's `frontmatter` attribute (if any).
/// The frontmatter is exposed under `markdoc.frontmatter` to match the
/// canonical `$markdoc.frontmatter.title` access pattern.
fn default_context_from(node: &Node) -> Context {
    let mut ctx = Context::new();
    if let Some(fm) = node.attributes.get("frontmatter") {
        ctx = ctx.with_frontmatter(fm.clone());
    }
    ctx
}

fn transform_node(
    node: &Node,
    config: &Config,
    ctx: &Context,
    fns: &HashMap<String, FunctionImpl>,
) -> Result<RenderableTreeNode> {
    // Inline interpolation `{% $var %}` / `{% func(...) %}` — evaluate the
    // captured expression against the context and emit the value as text. A
    // parse / evaluation failure (e.g. unknown function) drops to empty
    // rather than leaking the `{% … %}` source.
    if node.tag.as_deref() == Some(INTERPOLATION_TAG) {
        let value = node
            .expressions
            .get("primary")
            .and_then(|src| parse_expression(src).ok())
            .and_then(|expr| evaluate(&expr, ctx, fns).ok())
            .unwrap_or(Scalar::Null);
        return Ok(RenderableTreeNode::Scalar(Scalar::String(
            scalar_to_display(&value),
        )));
    }

    // Find schema for this node
    let schema = find_schema(node, config);

    // TODO If there's a custom transform function, use it
    // For now, we'll do basic transformation

    // Transform attributes
    let attributes = transform_attributes(node, config, ctx, &schema, fns)?;

    // Transform children
    let children = transform_children(node, config, ctx, fns)?;

    // Create tag or return scalar
    if should_render_as_tag(node, &schema) {
        let tag_name = get_render_name(node, &schema);
        Ok(RenderableTreeNode::Tag(Box::new(Tag {
            name: tag_name,
            attributes,
            children,
        })))
    } else {
        // Return as scalar if appropriate
        if children.is_empty() {
            if let Some(Scalar::String(content)) = node.attributes.get("content") {
                // Inline `code` keeps its identity as a `code` tag (its text
                // as a single child) so downstream renderers can style it —
                // flattened to a bare string it would be indistinguishable
                // from the surrounding prose. Other content-only nodes
                // (e.g. `text`) still flatten.
                if node.node_type == NodeType::Code {
                    Ok(RenderableTreeNode::Tag(Box::new(Tag {
                        name: "code".to_string(),
                        attributes: HashMap::new(),
                        children: vec![RenderableTreeNode::Scalar(Scalar::String(content.clone()))],
                    })))
                } else {
                    Ok(RenderableTreeNode::Scalar(Scalar::String(content.clone())))
                }
            } else {
                Ok(RenderableTreeNode::Scalar(Scalar::Null))
            }
        } else if children.len() == 1 {
            Ok(children[0].clone())
        } else {
            Ok(RenderableTreeNode::Scalar(Scalar::Array(
                children
                    .into_iter()
                    .map(|c| match c {
                        RenderableTreeNode::Scalar(s) => s,
                        _ => Scalar::Null,
                    })
                    .collect(),
            )))
        }
    }
}

/// Flatten a resolved scalar to display text for inline interpolation.
/// Whole-number floats (YAML integers) print without a trailing `.0`;
/// arrays / objects / null contribute nothing.
fn scalar_to_display(s: &Scalar) -> String {
    match s {
        Scalar::String(v) => v.clone(),
        Scalar::Number(n) if n.is_finite() && n.fract() == 0.0 => format!("{}", *n as i64),
        Scalar::Number(n) => n.to_string(),
        Scalar::Boolean(b) => b.to_string(),
        Scalar::Null | Scalar::Array(_) | Scalar::Object(_) => String::new(),
    }
}

fn find_schema(node: &Node, config: &Config) -> Option<Schema> {
    // First check if it's a tag
    if let Some(tag) = &node.tag
        && let Some(schema) = config.tags.get(tag)
    {
        return Some(schema.clone());
    }

    // Then check nodes
    let node_key = node.node_type.to_string();
    config.nodes.get(&node_key).cloned()
}

fn transform_attributes(
    node: &Node,
    _config: &Config,
    ctx: &Context,
    schema: &Option<Schema>,
    fns: &HashMap<String, FunctionImpl>,
) -> Result<HashMap<String, Scalar>> {
    let mut transformed = HashMap::new();

    // 1. Literal attributes (already-resolved scalars from the parser).
    for (key, value) in &node.attributes {
        place_attribute(&mut transformed, schema, key, value.clone());
    }

    // 2. Unresolved expressions (variables / function calls). Evaluate
    //    each against the context and merge the result into the same
    //    output map. Expression keys never collide with literal keys
    //    (the parser routes each attribute into one bucket or the other).
    for (key, source) in &node.expressions {
        let expr = parse_expression(source).map_err(|e| {
            MarkdocError::TransformError(format!(
                "Failed to parse expression for attribute {key:?} ({source:?}): {e}"
            ))
        })?;
        let value = evaluate(&expr, ctx, fns)?;
        place_attribute(&mut transformed, schema, key, value);
    }

    // 3. Schema defaults — applied only when the *source* supplied neither
    //    a literal nor an expression for the key. (Checking the output map
    //    here would be wrong: an attribute dropped via `render: false`
    //    would be re-added from its default.)
    if let Some(schema_inner) = schema
        && let Some(attrs) = &schema_inner.attributes
    {
        for (key, attr_schema) in attrs {
            let source_provided =
                node.attributes.contains_key(key) || node.expressions.contains_key(key);
            if source_provided {
                continue;
            }
            if let Some(default) = &attr_schema.default {
                place_attribute(&mut transformed, schema, key, default.clone());
            }
        }
    }

    Ok(transformed)
}

/// Insert `value` into `out` under `key`, honouring the schema's `render`
/// directive when the schema knows about that key (drop / rename).
fn place_attribute(
    out: &mut HashMap<String, Scalar>,
    schema: &Option<Schema>,
    key: &str,
    value: Scalar,
) {
    if let Some(schema) = schema
        && let Some(attrs) = &schema.attributes
        && let Some(attr_schema) = attrs.get(key)
    {
        match &attr_schema.render {
            Some(SchemaRender::Bool(false)) => return,
            Some(SchemaRender::String(render_key)) => {
                out.insert(render_key.clone(), value);
                return;
            }
            _ => {}
        }
    }
    out.insert(key.to_string(), value);
}

fn transform_children(
    node: &Node,
    config: &Config,
    ctx: &Context,
    fns: &HashMap<String, FunctionImpl>,
) -> Result<Vec<RenderableTreeNode>> {
    let mut children = Vec::new();

    for child in &node.children {
        children.push(transform_node(child, config, ctx, fns)?);
    }

    Ok(children)
}

fn should_render_as_tag(node: &Node, schema: &Option<Schema>) -> bool {
    // Custom Markdoc tags always render as tags.
    if node.tag.is_some() {
        return true;
    }

    // A schema with an explicit render hint forces tag rendering.
    if let Some(schema) = schema
        && schema.render.is_some()
    {
        return true;
    }

    // Otherwise: structural markdown nodes always render as tags, with or
    // without a schema. (Previously this branch was unreachable when a
    // schema was present even with `render: None`, dropping headings/lists
    // out of the rendered tree once the default config registered them.)
    matches!(
        node.node_type,
        NodeType::Document
            | NodeType::Heading
            | NodeType::Paragraph
            | NodeType::Blockquote
            | NodeType::List
            | NodeType::Item
            | NodeType::Table
            | NodeType::Thead
            | NodeType::Tbody
            | NodeType::Tr
            | NodeType::Th
            | NodeType::Td
            | NodeType::Link
            | NodeType::Image
            | NodeType::Em
            | NodeType::Strong
            | NodeType::Strikethrough
            | NodeType::Fence
            | NodeType::Hr
            // Soft and hard breaks must survive transform so downstream
            // renderers can decide their semantics — a soft break is
            // typically a space, a hard break a forced line break.
            // Without these branches they collapse to `Scalar::Null`
            // and disappear from the rendered tree, silently dropping
            // the inter-word whitespace they represent.
            | NodeType::Softbreak
            | NodeType::Hardbreak
    )
}

fn get_render_name(node: &Node, schema: &Option<Schema>) -> String {
    // Use tag name if available
    if let Some(tag) = &node.tag {
        return tag.clone();
    }

    // Use schema render name
    if let Some(schema) = schema
        && let Some(render) = &schema.render
    {
        return render.clone();
    }

    // Use default HTML tag names
    match node.node_type {
        NodeType::Heading => {
            if let Some(Scalar::Number(level)) = node.attributes.get("level") {
                format!("h{}", *level as i32)
            } else {
                "h1".to_string()
            }
        }
        NodeType::Paragraph => "p".to_string(),
        NodeType::Blockquote => "blockquote".to_string(),
        NodeType::List => {
            if let Some(Scalar::Boolean(true)) = node.attributes.get("ordered") {
                "ol".to_string()
            } else {
                "ul".to_string()
            }
        }
        NodeType::Item => "li".to_string(),
        NodeType::Link => "a".to_string(),
        NodeType::Image => "img".to_string(),
        NodeType::Em => "em".to_string(),
        NodeType::Strong => "strong".to_string(),
        NodeType::Strikethrough => "s".to_string(),
        NodeType::Code => "code".to_string(),
        NodeType::Fence => "pre".to_string(),
        NodeType::Table => "table".to_string(),
        NodeType::Thead => "thead".to_string(),
        NodeType::Tbody => "tbody".to_string(),
        NodeType::Tr => "tr".to_string(),
        NodeType::Th => "th".to_string(),
        NodeType::Td => "td".to_string(),
        NodeType::Hr => "hr".to_string(),
        NodeType::Hardbreak => "br".to_string(),
        NodeType::Softbreak => "softbreak".to_string(),
        _ => "div".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn find_tag<'a>(node: &'a RenderableTreeNode, name: &str) -> Option<&'a Tag> {
        match node {
            RenderableTreeNode::Tag(t) if t.name == name => Some(t),
            RenderableTreeNode::Tag(t) => {
                for child in &t.children {
                    if let Some(found) = find_tag(child, name) {
                        return Some(found);
                    }
                }
                None
            }
            _ => None,
        }
    }

    #[test]
    fn variable_attribute_resolves_via_context() {
        // {% callout type=$config.severity %} with config.severity="warning"
        let src = "{% callout type=$config.severity %}body{% /callout %}";
        let doc = parse(src, None).unwrap();
        let mut cfg = HashMap::new();
        cfg.insert(
            "severity".to_string(),
            Scalar::String("warning".to_string()),
        );
        let ctx = Context::new().with_config(Scalar::Object(cfg));
        let rendered = transform_with_context(&doc, &Config::default(), &ctx).unwrap();
        let callout = find_tag(&rendered, "callout").expect("callout in tree");
        assert_eq!(
            callout.attributes.get("type"),
            Some(&Scalar::String("warning".to_string()))
        );
    }

    #[test]
    fn function_call_attribute_resolves() {
        // {% callout active=equals($config.mode, "on") %} with config.mode="on" → true
        let src = r#"{% callout active=equals($config.mode, "on") %}x{% /callout %}"#;
        let doc = parse(src, None).unwrap();
        let mut cfg = HashMap::new();
        cfg.insert("mode".to_string(), Scalar::String("on".to_string()));
        let ctx = Context::new().with_config(Scalar::Object(cfg));
        let rendered = transform_with_context(&doc, &Config::default(), &ctx).unwrap();
        let callout = find_tag(&rendered, "callout").unwrap();
        assert_eq!(
            callout.attributes.get("active"),
            Some(&Scalar::Boolean(true))
        );
    }

    #[test]
    fn missing_variable_resolves_to_null_attribute() {
        let src = "{% callout type=$config.never_set %}x{% /callout %}";
        let doc = parse(src, None).unwrap();
        let rendered = transform_with_context(&doc, &Config::default(), &Context::new()).unwrap();
        let callout = find_tag(&rendered, "callout").unwrap();
        assert_eq!(callout.attributes.get("type"), Some(&Scalar::Null));
    }

    #[test]
    fn frontmatter_resolves_via_default_context() {
        // The shim `transform()` should build a context from the document's
        // frontmatter automatically.
        let src = r#"---
title: Hello
---

{% callout heading=$markdoc.frontmatter.title %}body{% /callout %}"#;
        let doc = parse(src, None).unwrap();
        let rendered = transform(&doc, &Config::default()).unwrap();
        let callout = find_tag(&rendered, "callout").unwrap();
        assert_eq!(
            callout.attributes.get("heading"),
            Some(&Scalar::String("Hello".to_string()))
        );
    }

    #[test]
    fn literal_attributes_pass_through_unchanged() {
        let src = r#"{% callout type="warning" count=42 %}x{% /callout %}"#;
        let doc = parse(src, None).unwrap();
        let rendered = transform_with_context(&doc, &Config::default(), &Context::new()).unwrap();
        let callout = find_tag(&rendered, "callout").unwrap();
        assert_eq!(
            callout.attributes.get("type"),
            Some(&Scalar::String("warning".to_string()))
        );
        assert_eq!(callout.attributes.get("count"), Some(&Scalar::Number(42.0)));
    }

    fn collect_text(node: &RenderableTreeNode, out: &mut String) {
        match node {
            RenderableTreeNode::Scalar(Scalar::String(s)) => out.push_str(s),
            RenderableTreeNode::Tag(t) => {
                for c in &t.children {
                    collect_text(c, out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn frontmatter_interpolation_resolves_at_transform() {
        // `{% $markdoc.frontmatter.* %}` is an interpolation node after parse;
        // `transform` builds a Context from the document's frontmatter and
        // resolves it to text.
        let src = "---\ntitle: Hello World\n---\n\n# {% $markdoc.frontmatter.title %}";
        let doc = parse(src, None).unwrap();
        let rendered = transform(&doc, &Config::default()).unwrap();
        let mut text = String::new();
        collect_text(&rendered, &mut text);
        assert!(text.contains("Hello World"), "got: {text:?}");
    }

    #[test]
    fn function_interpolation_resolves_at_transform() {
        // A function call in an inline interpolation resolves too — and a
        // whole-number frontmatter value stringifies without a `.0`.
        let src = "---\ndocumentNumber: 1234567\n---\n\n{% default($markdoc.frontmatter.documentNumber, \"x\") %}";
        let doc = parse(src, None).unwrap();
        let rendered = transform(&doc, &Config::default()).unwrap();
        let mut text = String::new();
        collect_text(&rendered, &mut text);
        assert!(text.contains("1234567"), "got: {text:?}");
    }
}
