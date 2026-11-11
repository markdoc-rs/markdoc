//! Port of upstream Markdoc v0.5.11's `src/tags/index.ts`.
//!
//! Owner: the semantics agent.
//!
//! The default tags.

pub mod conditional;
pub mod partial;
pub mod slot;
pub mod table;

use crate::next::schema::Schema;
use indexmap::IndexMap;
use std::sync::LazyLock;

static DEFAULT_TAGS: LazyLock<IndexMap<String, Option<Schema>>> = LazyLock::new(|| {
    [
        ("else", conditional::tag_else()),
        ("if", conditional::tag_if()),
        ("partial", partial::partial()),
        ("slot", slot::slot()),
        ("table", table::table()),
    ]
    .into_iter()
    .map(|(name, schema)| (name.to_string(), Some(schema)))
    .collect()
});

/// Upstream's `tags`: `else`, `if`, `partial`, `slot`, `table`.
pub fn default_tags() -> &'static IndexMap<String, Option<Schema>> {
    &DEFAULT_TAGS
}
