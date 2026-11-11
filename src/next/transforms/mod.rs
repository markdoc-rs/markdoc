//! Port of upstream Markdoc v0.5.11's `src/transforms/index.ts`.
//!
//! Owner: the parser agent.
//!
//! The list of AST transforms `parse` runs, in order, on the parsed document.

use crate::ast::Node;
use crate::types::Result;

pub mod table;

/// A transform: the document, and `ParserArgs.conditionalTags` (`None` when not given, so
/// each transform applies its own default). `Err` where upstream's throws: the walk's
/// `RangeError` past [`crate::next::parser::MAX_AST_DEPTH`].
pub type Transform = fn(&mut Node, Option<&[String]>) -> Result<()>;

/// `export default [table]`.
pub const TRANSFORMS: [Transform; 1] = [table::transform];
