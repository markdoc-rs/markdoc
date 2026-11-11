//! Port of upstream Markdoc v0.5.11's `src/tags/conditional.ts`.
//!
//! Owner: the semantics agent.

use crate::next::schema::value::{self, Value};
use crate::next::schema::{Config, MarkdocType, Schema, SchemaAttribute, TransformFn, TypeSpec};
use crate::next::transformer::{self, ResolvedNode, flat_push};
use crate::types::{NodeType, Result};
use indexmap::IndexMap;
use std::sync::Arc;

/// `truthy(value)`: anything but `false`, `undefined` and `null`.
pub fn truthy(value: &Value) -> bool {
    value::truthy(value)
}

/// `renderConditions(node)`: the `if`'s own condition and the children up to the first
/// `else`, then each `else`'s condition (`'primary' in child.attributes ?
/// child.attributes.primary : true`, through the attributes' prototype) and the children
/// after it.
#[allow(clippy::type_complexity)]
fn render_conditions<'n, 'a>(
    node: &'n ResolvedNode<'a>,
) -> Result<Vec<(Value, Vec<&'n ResolvedNode<'a>>)>> {
    let mut conditions = vec![(node.attribute("primary")?, Vec::new())];
    for child in &node.children {
        if child.node.node_type == NodeType::Tag && child.node.tag.as_deref() == Some("else") {
            let condition = if value::has_property(&child.attributes, "primary")
                .map_err(crate::types::MarkdocError::TransformError)?
            {
                child.attribute("primary")?
            } else {
                Value::Boolean(true)
            };
            conditions.push((condition, Vec::new()));
        } else {
            conditions
                .last_mut()
                .expect("the if's own condition is first")
                .1
                .push(child);
        }
    }
    Ok(conditions)
}

/// The `if` tag's transform: the children of the first condition that is truthy.
fn transform_if(node: &ResolvedNode<'_>, config: &Config) -> Result<Value> {
    // `children.flatMap((child) => child.transform(config))`: what it costs node's stack.
    let _level = transformer::charge(transformer::cost::IF)?;
    let mut nodes = Vec::new();
    for child in chosen(node)? {
        flat_push(&mut nodes, child.transform(config)?);
    }
    Ok(Value::array(nodes))
}

/// The children of the first truthy condition (none if no condition is). (A function of its
/// own so that the frame [`transform_if`] keeps on the stack for each level of nested `if`s
/// stays small.)
#[inline(never)]
fn chosen<'n, 'a>(node: &'n ResolvedNode<'a>) -> Result<Vec<&'n ResolvedNode<'a>>> {
    Ok(render_conditions(node)?
        .into_iter()
        .find(|(condition, _)| truthy(condition))
        .map(|(_, children)| children)
        .unwrap_or_default())
}

fn primary() -> IndexMap<String, SchemaAttribute> {
    let mut attributes = IndexMap::new();
    attributes.insert(
        "primary".to_string(),
        SchemaAttribute {
            attr_type: Some(TypeSpec::Markdoc(MarkdocType::ConditionalAttributeType)),
            render: Some(Value::Boolean(false)),
            ..Default::default()
        },
    );
    attributes
}

/// `tagIf`.
pub fn tag_if() -> Schema {
    let transform: TransformFn = Arc::new(transform_if);
    Schema {
        attributes: Some(primary()),
        transform: Some(transform),
        ..Default::default()
    }
}

/// `tagElse`.
pub fn tag_else() -> Schema {
    Schema {
        self_closing: true,
        attributes: Some(primary()),
        ..Default::default()
    }
}
