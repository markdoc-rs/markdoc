//! Port of upstream Markdoc v0.5.11's `src/tags/slot.ts`.
//!
//! Owner: the semantics agent.

use crate::next::schema::{BuiltinType, Schema, SchemaAttribute, TypeSpec};
use indexmap::IndexMap;

/// `slot`.
pub fn slot() -> Schema {
    let mut attributes = IndexMap::new();
    attributes.insert(
        "primary".to_string(),
        SchemaAttribute {
            attr_type: Some(TypeSpec::Builtin(BuiltinType::String)),
            required: true,
            ..Default::default()
        },
    );
    Schema {
        attributes: Some(attributes),
        ..Default::default()
    }
}
