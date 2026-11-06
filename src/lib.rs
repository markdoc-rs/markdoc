pub mod ast;
pub mod conditionals;
pub mod crossrefs;
pub mod expression;
pub mod footnotes;
pub mod frontmatter;
pub mod functions;
pub mod list_table;
pub mod parser;
pub mod partials;
pub mod renderers;
pub mod schema;
pub mod sourcemap;
pub mod tag_parser;
pub mod tags;
pub mod tokenizer;
pub mod transformer;
pub mod types;
pub mod validator;

pub use ast::{Function, Node, Variable};
pub use conditionals::evaluate_conditionals;
pub use crossrefs::{AnchorInfo, collect_anchors, resolve_crossrefs};
pub use footnotes::resolve_footnotes;
pub use parser::parse;
pub use partials::{FsPartialResolver, InMemoryPartialResolver, PartialResolver, expand_partials};
pub use transformer::{transform, transform_with_context};
pub use types::Context;
pub use validator::validate;

use types::*;

/// Main Markdoc struct for parsing and transforming markdown documents
#[derive(Default)]
pub struct Markdoc {
    config: Config,
}

impl Markdoc {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn parse(&self, content: &str) -> Result<ast::Node> {
        parse(content, None)
    }

    pub fn transform(&self, node: &ast::Node) -> Result<RenderableTreeNode> {
        transform(node, &self.config)
    }

    pub fn transform_with_context(
        &self,
        node: &ast::Node,
        ctx: &Context,
    ) -> Result<RenderableTreeNode> {
        transform_with_context(node, &self.config, ctx)
    }

    pub fn validate(&self, node: &ast::Node) -> Vec<ValidationError> {
        validate(node, &self.config)
    }
}
