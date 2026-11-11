//! Port of upstream Markdoc v0.5.11's `src/tags/partial.ts`.
//!
//! Owner: the semantics agent.
//!
//! The partial's children are transformed as upstream's `transformChildren` gives them:
//! flattened once, a part after another for a list of partials (`partials: {p: [a, b]}`), and
//! spliced into the parent by its own `flatMap`, so an array a partial's content renders to
//! stays an array (`{% $arr %}` with `arr: [[1, 2]]` and `paragraph: null` gives the article
//! the child `[1, 2]`, not `1` and `2`).
//!
//! # Divergences from upstream
//!
//! - Depth. A partial costs the depth budget what its frames cost node's stack
//!   ([`crate::next::transformer::cost::PARTIAL`]), so partials that each include the next
//!   throw upstream's `RangeError` past the 903rd, as a fresh node 22 process does (1,840 warmed
//!   up), and the partial's content is resolved and transformed with what is left of the
//!   budget where it is included, as upstream does it on the stack its transform is using.
//!   Where the stack cannot grow ([`crate::depth::can_grow`]), a partial is a level of the
//!   transform's fixed depth of its own besides its tag's
//!   ([`crate::next::transformer::MAX_TRANSFORM_DEPTH`]), so the 550th throws. A partial
//!   including itself throws in both.

use crate::next::schema::value::{self, Value, scalar_to_js_string};
use crate::next::schema::{
    BuiltinType, Config, Found, MarkdocType, Partial, Schema, SchemaAttribute, TransformFn,
    TypeSpec, Variables, lookup,
};
use crate::next::transformer::{self, ResolvedNode, children, resolve_node};
use crate::types::{JsObject, MarkdocError, Result, Scalar, ValidationError, ValidationLevel};
use indexmap::IndexMap;
use std::sync::Arc;

/// Is the partial an entry upstream's `if (!partial)` lets through?
fn present(found: &Found<'_, Partial>) -> bool {
    match found {
        Found::Own(Partial::Value(v)) => v.to_boolean(),
        Found::Own(_) | Found::Inherited => true,
        Found::Missing => false,
    }
}

/// `partials[file]` with `const { partials = {} } = config` (a `null` kept: reading from it
/// throws).
fn find<'c>(config: &'c Config, key: &str) -> std::result::Result<Found<'c, Partial>, String> {
    static NONE: std::sync::LazyLock<IndexMap<String, Partial>> =
        std::sync::LazyLock::new(IndexMap::new);
    if config.partials_null {
        return Err(format!(
            "TypeError: Cannot read properties of null (reading '{key}')"
        ));
    }
    Ok(lookup(config.partials.as_ref().unwrap_or(&NONE), key))
}

/// `PartialFile.validate(file, config)`: the file must be in `config.partials`. `Err` where
/// `partials[file]` throws (a file that cannot be made a string).
pub fn partial_file_validate(
    file: &Scalar,
    config: &Config,
) -> std::result::Result<Vec<ValidationError>, String> {
    let name = scalar_to_js_string(file)?;
    if !present(&find(config, &name)?) {
        return Ok(vec![ValidationError {
            id: "attribute-value-invalid".into(),
            level: ValidationLevel::Error,
            message: format!(
                "Partial `{name}` not found. The 'file' attribute must be set in `config.partials`"
            ),
            location: None,
            raw: None,
            span: None,
        }]);
    }
    Ok(Vec::new())
}

fn throw(message: impl Into<String>) -> MarkdocError {
    MarkdocError::TransformError(message.into())
}

/// What [`transform_partial`] does before it transforms: its value already, or the parts to
/// transform under the scoped config.
enum Prepared<'c> {
    Done(Value),
    Parts(Box<Config>, Vec<&'c crate::ast::Node>),
}

/// The partial tag's transform: the partial's children, resolved and transformed with the
/// tag's `variables` and `$$partial:filename` over the config's variables.
fn transform_partial(node: &ResolvedNode<'_>, config: &Config) -> Result<Value> {
    let (scoped_config, parts) = match prepare(node, config)? {
        Prepared::Done(value) => return Ok(value),
        Prepared::Parts(scoped_config, parts) => (scoped_config, parts),
    };

    // What the partial's own frames cost node's stack (`transformer::cost::PARTIAL`; a level
    // of the transform's depth of its own where the stack cannot grow, see
    // `transformer::MAX_TRANSFORM_DEPTH`): a partial including itself throws upstream's
    // `RangeError` instead of overflowing the stack.
    let _depth = transformer::enter_transform(transformer::cost::PARTIAL)?;

    // `transformChildren(partial)`, or `partial.flatMap(transformChildren)` for a list: each
    // part's children, already flattened once by `transformChildren`, one list after another.
    let mut out = Vec::new();
    for part in parts {
        let resolved = resolve_node(part, &scoped_config)?;
        out.extend(children(&resolved, &scoped_config)?);
    }
    Ok(Value::array(out))
}

/// The partial tag's transform up to the transforming: the partial (`null` if there is none),
/// the scoped config, the parts. (A function of its own so that the frame
/// [`transform_partial`] keeps on the stack while the partial transforms stays small.)
#[inline(never)]
fn prepare<'c>(node: &ResolvedNode<'_>, config: &'c Config) -> Result<Prepared<'c>> {
    // `const { file, variables } = node.attributes;`, through the attributes' prototype (a
    // `null` or `undefined` the attributes resolved to cannot be destructured: V8 names the
    // dist's expression).
    if node.attributes.is_nullish() {
        return Err(throw(format!(
            "TypeError: Cannot destructure property 'file' of 'node2.attributes' as it is {}.",
            if node.attributes.is_undefined() {
                "undefined"
            } else {
                "null"
            }
        )));
    }
    let file = node.attribute("file")?;
    let variables = node.attribute("variables")?;
    // `partials[file]`: a key with a lone surrogate (from a variable path into a string) is
    // no key a config's map holds.
    let found = match file.to_js_string().map_err(throw)?.as_str() {
        Some(key) => find(config, key).map_err(throw)?,
        None => Found::Missing,
    };

    if !present(&found) {
        return Ok(Prepared::Done(Value::Null));
    }

    // `{...config.variables, ...variables, ['$$partial:filename']: file}`: a JsObject, so a
    // spread of n index keys (a string's code units, an array's items) after named ones is
    // O(n log n), as in V8, not a shift per key.
    let mut scoped_variables = JsObject::new();
    if let Some(Variables::Value(v)) = &config.variables {
        scoped_variables.extend(value::own_entries(v));
    }
    scoped_variables.extend(value::own_entries(&variables));
    scoped_variables.insert("$$partial:filename", file);
    let mut scoped_config = config.clone();
    scoped_config.variables = Some(Variables::Value(Value::object(scoped_variables)));

    let parts: Vec<&crate::ast::Node> = match found {
        Found::Own(Partial::Node(part)) => vec![part.as_ref()],
        Found::Own(Partial::Nodes(parts)) => parts.iter().collect(),
        // `partial.flatMap(transformChildren)` over no items.
        Found::Own(Partial::Value(Value::Array(items))) if items.is_empty() => {
            return Ok(Prepared::Done(Value::array(Vec::new())));
        }
        // A string, a number, an object, a built-in function: `part.resolve` is not a function.
        Found::Own(Partial::Value(_)) | Found::Inherited => {
            return Err(throw("TypeError: part.resolve is not a function"));
        }
        Found::Missing => unreachable!("not present"),
    };
    Ok(Prepared::Parts(Box::new(scoped_config), parts))
}

/// `partial`.
pub fn partial() -> Schema {
    let mut attributes = IndexMap::new();
    attributes.insert(
        "file".to_string(),
        SchemaAttribute {
            attr_type: Some(TypeSpec::Markdoc(MarkdocType::PartialFile)),
            render: Some(Value::Boolean(false)),
            required: true,
            ..Default::default()
        },
    );
    attributes.insert(
        "variables".to_string(),
        SchemaAttribute {
            attr_type: Some(TypeSpec::Builtin(BuiltinType::Object)),
            render: Some(Value::Boolean(false)),
            ..Default::default()
        },
    );
    let transform: TransformFn = Arc::new(transform_partial);
    Schema {
        inline: Some(Value::Boolean(false)),
        self_closing: true,
        attributes: Some(attributes),
        transform: Some(transform),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Node, Variable};
    use crate::next::index;
    use crate::types::{NodeType, RenderableTreeNode};

    fn text_of(variable: &str) -> Node {
        let content = Scalar::Variable(Variable::new(vec![variable.into()]));
        Node::new(
            NodeType::Text,
            vec![("content".to_string(), content)],
            vec![],
            None,
        )
    }

    fn partial_tag(file: &str, variables: Option<Scalar>) -> Node {
        let mut attributes = vec![("file".to_string(), Scalar::String(file.into()))];
        if let Some(v) = variables {
            attributes.push(("variables".to_string(), v));
        }
        Node::new(NodeType::Tag, attributes, vec![], Some("partial".into()))
    }

    fn document(children: Vec<Node>) -> Node {
        Node::new(
            NodeType::Document,
            Vec::<(String, Scalar)>::new(),
            children,
            None,
        )
    }

    #[test]
    fn a_list_of_partials_renders_each_with_the_scoped_variables() {
        let x = Scalar::Object(IndexMap::from([("x".to_string(), Scalar::Number(1.0))]));
        let doc = document(vec![partial_tag("a", Some(x))]);
        let parts = vec![
            document(vec![text_of("x")]),
            document(vec![text_of("$$partial:filename")]),
        ];
        let config = Config {
            partials: Some(IndexMap::from([(
                "a".to_string(),
                Partial::Nodes(Arc::new(parts)),
            )])),
            ..Default::default()
        };
        let RenderableTreeNode::Tag(article) = index::transform(&doc, &config)
            .unwrap()
            .to_render()
            .unwrap()
        else {
            panic!("an article")
        };
        assert_eq!(
            article.children,
            vec![
                RenderableTreeNode::Scalar(Scalar::Number(1.0)),
                RenderableTreeNode::Scalar(Scalar::String("a".into())),
            ]
        );
    }

    #[test]
    fn a_partial_that_includes_itself_throws_instead_of_overflowing() {
        let looping = document(vec![partial_tag("self", None)]);
        let config = Config {
            partials: Some(IndexMap::from([(
                "self".to_string(),
                Partial::Node(Arc::new(looping.clone())),
            )])),
            ..Default::default()
        };
        assert_eq!(
            index::transform(&looping, &config).unwrap_err().to_string(),
            "Transform error: RangeError: Maximum call stack size exceeded"
        );
        // ... and leaves the depth as it found it: the same transform throws the same way.
        assert!(index::transform(&looping, &config).is_err());
    }

    #[test]
    fn a_missing_or_empty_partial_renders_null() {
        let doc = document(vec![partial_tag("none", None), partial_tag("empty", None)]);
        let config = Config {
            partials: Some(IndexMap::from([(
                "empty".to_string(),
                Partial::Value(Value::from("")),
            )])),
            ..Default::default()
        };
        let RenderableTreeNode::Tag(article) = index::transform(&doc, &config)
            .unwrap()
            .to_render()
            .unwrap()
        else {
            panic!("an article")
        };
        assert_eq!(
            article.children,
            vec![RenderableTreeNode::Scalar(Scalar::Null); 2]
        );
    }

    /// Upstream's AST of `{% $name %}` alone (a paragraph, its inline, the interpolation).
    fn interpolation(name: &str) -> serde_json::Value {
        serde_json::json!({"type": "document", "children": [{"type": "paragraph", "children": [
            {"type": "inline", "children": [{"type": "text", "inline": true,
                "attributes": {"content": {"$$mdtype": "Variable", "path": [name]}}}]}]}]})
    }

    /// `Markdoc.transform(ast, {...config, partials})`, as JSON.
    fn transform_json(
        ast: serde_json::Value,
        partials: Vec<(&str, serde_json::Value)>,
        config: serde_json::Value,
    ) -> serde_json::Value {
        use crate::next::compare::{node_from_json, render_json};
        let mut config = Config::from_json(&config).unwrap();
        config.partials = Some(
            partials
                .into_iter()
                .map(|(name, p)| {
                    let node = node_from_json(&p).unwrap();
                    (name.to_string(), Partial::Node(Arc::new(node)))
                })
                .collect(),
        );
        let ast = node_from_json(&ast).unwrap();
        render_json(
            &index::transform(&ast, &config)
                .unwrap()
                .to_render()
                .unwrap(),
        )
    }

    #[test]
    fn a_partials_children_are_flattened_once() {
        use serde_json::json;
        // node: Markdoc.transform(Markdoc.parse(doc), {...config, partials: {p: Markdoc.parse(
        // '{% $arr %}')}}), the article's children: an array a variable gives stays an item
        // wherever no render name wraps it.
        let partial = || json!({"type": "tag", "tag": "partial", "attributes": {"file": "p"}});
        let article = |children: serde_json::Value| json!({"$$mdtype": "Tag", "name": "article", "attributes": {}, "children": children});
        let variables = json!({"arr": [[1, 2]], "deep": [[[1]], 2]});
        for (paragraph, wrap, expected) in [
            // `{% partial file="p" /%}` with paragraphs not rendered: [[1, 2]].
            (json!(null), None, json!([[1, 2]])),
            (json!({}), None, json!([[1, 2]])),
            // Inside `{% if true %}` and a tag without a render name.
            (
                json!(null),
                Some(("if", json!({"primary": true}))),
                json!([[1, 2]]),
            ),
            (json!(null), Some(("w", json!({}))), json!([[1, 2]])),
            // Paragraphs rendered: the array is the `p`'s child.
            (
                json!({"render": "p"}),
                None,
                json!([{"$$mdtype": "Tag", "name": "p", "attributes": {}, "children": [[1, 2]]}]),
            ),
        ] {
            let mut doc_child = partial();
            if let Some((tag, attributes)) = &wrap {
                doc_child = json!({"type": "tag", "tag": tag, "attributes": attributes,
                    "children": [doc_child]});
            }
            let doc = json!({"type": "document", "children": [doc_child]});
            let config = json!({"nodes": {"paragraph": paragraph}, "tags": {"w": {}},
                "variables": variables});
            assert_eq!(
                transform_json(doc, vec![("p", interpolation("arr"))], config),
                article(expected),
                "paragraph {paragraph}, inside {wrap:?}"
            );
        }
        // Two paragraphs in the partial, `{% $arr %}` and `{% $deep %}`: [[1, 2], [[1]], 2].
        let mut two = interpolation("arr");
        let second = interpolation("deep")["children"][0].clone();
        two["children"].as_array_mut().unwrap().push(second);
        let doc = json!({"type": "document", "children": [partial()]});
        let config = json!({"nodes": {"paragraph": null}, "variables": variables});
        assert_eq!(
            transform_json(doc, vec![("p", two)], config),
            article(json!([[1, 2], [[1]], 2]))
        );
    }

    #[test]
    fn a_list_of_partials_is_flattened_once_too() {
        // node: `partials: {p: [Markdoc.parse('{% $arr %}'), Markdoc.parse('{% $arr %}')]}`
        // with `paragraph: null` and `arr: [[1, 2]]`: the article's children [[1, 2], [1, 2]].
        use crate::next::compare::{node_from_json, render_json};
        let part = node_from_json(&interpolation("arr")).unwrap();
        let mut config = Config::from_json(&serde_json::json!({"nodes": {"paragraph": null},
            "variables": {"arr": [[1, 2]]}}))
        .unwrap();
        config.partials = Some(IndexMap::from([(
            "p".to_string(),
            Partial::Nodes(Arc::new(vec![part.clone(), part])),
        )]));
        let doc = document(vec![partial_tag("p", None)]);
        assert_eq!(
            render_json(
                &index::transform(&doc, &config)
                    .unwrap()
                    .to_render()
                    .unwrap()
            )["children"],
            serde_json::json!([[1, 2], [1, 2]])
        );
    }
}
