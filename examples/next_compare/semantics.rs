//! The transform, errors and html stages' way into `markdoc::next::{index, renderers}`.

use indexmap::IndexMap;
use markdoc::ast::Node;
use markdoc::next::compare::ValidateRecord;
use markdoc::next::index;
use markdoc::next::renderers::html as html_renderer;
use markdoc::next::schema::{Config, Partial, Value};
use markdoc::types::RenderableTreeNode;
use std::cell::RefCell;
use std::sync::Arc;

thread_local! {
    /// The current request's `config.variables` read exactly where its JSON holds a lone
    /// surrogate (serde_json cannot): they replace what `Config::from_json` read from the
    /// lossy request, so variables reach the values as JS holds them.
    static EXACT_VARIABLES: RefCell<Option<Value>> = const { RefCell::new(None) };
}

/// Sets the current request's exact `config.variables` (see [`EXACT_VARIABLES`]).
pub fn set_exact_variables(variables: Option<Value>) {
    EXACT_VARIABLES.with(|v| *v.borrow_mut() = variables);
}

/// `Config::from_json`, with the request's exact `variables` where it has them.
fn config_from_json(config: &serde_json::Value) -> Result<Config, String> {
    let mut config = Config::from_json(config)?;
    if config.variables.is_some()
        && let Some(exact) = EXACT_VARIABLES.with(|v| v.borrow().clone())
    {
        config.variables = Some(markdoc::next::schema::Variables::Value(exact));
    }
    Ok(config)
}

/// `Markdoc.transform(ast, {...config, partials})`: the render tree as values, as upstream
/// returns it (its JSON is `Value::to_render`). `config` is the request's JSON config (types
/// as strings, `{"$regex", "flags"}` for a RegExp, `{"$type": name}` for a constructor or
/// class; see tests/next/oracle.cjs); `partials` are the partials' ASTs, parsed as the oracle
/// parses them.
pub fn transform(
    ast: &Node,
    config: &serde_json::Value,
    partials: &IndexMap<String, Node>,
) -> Result<Value, String> {
    let mut config = config_from_json(config)?;
    // The oracle transforms with the partials it parsed (`{}` for a `null`).
    config.partials_null = false;
    config.partials = Some(
        partials
            .iter()
            .map(|(name, node)| (name.clone(), Partial::Node(Arc::new(node.clone()))))
            .collect(),
    );
    index::transform(ast, &config).map_err(|e| index::thrown(&e).to_owned())
}

/// `Markdoc.validate(ast, config)`, each entry as `{type, lines, location, error}`. The config's
/// `partials` are what the request's JSON holds (the sources), as the oracle validates.
pub fn validate(ast: &Node, config: &serde_json::Value) -> Result<Vec<ValidateRecord>, String> {
    let config = config_from_json(config)?;
    let errors = index::validate(ast, &config).map_err(|e| index::thrown(&e).to_owned())?;
    Ok(errors
        .into_iter()
        .map(|e| ValidateRecord {
            node_type: e.node_type.to_string(),
            lines: e.lines,
            location: e.location.map(|l| l.to_json()),
            error: e.error,
        })
        .collect())
}

/// `Markdoc.renderers.html(tree)` of the values [`transform`] made.
pub fn html(tree: &Value) -> Result<String, String> {
    html_renderer::render(tree).map_err(|e| index::thrown(&e).to_owned())
}

/// `Markdoc.renderers.html(tree)` of a tree read from JSON (upstream's, with
/// `--from-oracle`).
pub fn html_tree(tree: &RenderableTreeNode) -> Result<String, String> {
    html_renderer::render_tree(tree).map_err(|e| index::thrown(&e).to_owned())
}
