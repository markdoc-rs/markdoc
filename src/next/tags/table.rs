//! Port of upstream Markdoc v0.5.11's `src/tags/table.ts`.
//!
//! Owner: the semantics agent.

use crate::next::schema::{Schema, Value};

/// `table`.
pub fn table() -> Schema {
    Schema {
        children: Some(Value::array(vec![Value::string("table")])),
        inline: Some(Value::Boolean(false)),
        ..Default::default()
    }
}
