//! Port of upstream Markdoc v0.5.11's `src/ast/index.ts` and `src/ast/base.ts`: what
//! `Markdoc.Ast` holds besides its classes (which are [`crate::ast::Node`],
//! [`crate::ast::Variable`] and [`crate::ast::Function`]): `fromJSON`, `isAst`, `isFunction`,
//! `isVariable`, `getAstValues` and `resolve`.
//!
//! Owner: the api agent.
//!
//! # `fromJSON`
//!
//! Upstream parses the text with `JSON.parse` and a reviver that makes every object whose
//! `$$mdtype` is `Node`, `Variable` or `Function` an instance of that class
//! (`Object.assign(new klass(), value)`: the class's own fields first, defaulted, then the
//! object's), at any depth. [`from_json`] gives the same tree: a [`Node`] (or a list of them)
//! where the JSON is one, read as `compare::node_from_json` reads upstream's AST JSON (a field
//! the JSON leaves out has the class's default), with every value in it a [`Scalar`] as
//! [`Scalar::from_json`] reads it (`Variable`, `Function`); any other JSON is a
//! [`Revived::Value`]. A `Node` object inside a value (an attribute's, say) stays a
//! [`Scalar::Object`] with its `$$mdtype`, which the formatter reads as the node it describes.
//!
//! # Divergences from upstream
//!
//! - Text that is not JSON: upstream throws V8's `SyntaxError` (`Unexpected token ...`,
//!   `Unexpected end of JSON input`, with V8's positions and context); here it is a
//!   `SyntaxError` whose message is serde_json's.
//! - A `Node` object whose fields have types upstream's class never gives them (a `children`
//!   that is not a list of nodes, a `type` that is not a string) cannot be an
//!   [`crate::ast::Node`]: [`from_json`] gives it as a [`Revived::Value`] (an object), which
//!   `transform` and `validate` cannot take; a `Variable` or `Function` object of such fields
//!   is `Err`.

use crate::ast::Node;
use crate::next::compare::node_from_json;
use crate::next::schema::Config;
use crate::next::schema::value::{self, Value};
use crate::next::stack;
use crate::types::{MarkdocError, RenderableTreeNode, Result, Scalar, Tag};
use std::borrow::Cow;

/// What [`from_json`] revives.
#[derive(Debug, Clone)]
pub enum Revived {
    /// The JSON of a node (`$$mdtype: 'Node'`).
    Node(Box<Node>),
    /// A list, every item the JSON of a node.
    Nodes(Vec<Node>),
    /// Anything else, as [`Scalar::from_json`] reads it.
    Value(Scalar),
}

/// `Ast.fromJSON(text)`: the AST the JSON describes (see the module's documentation).
pub fn from_json(text: &str) -> Result<Revived> {
    let json: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| MarkdocError::ParseError(format!("SyntaxError: {e}")))?;
    from_json_value(&json)
}

/// [`from_json`] of JSON already parsed.
pub fn from_json_value(json: &serde_json::Value) -> Result<Revived> {
    let is_node =
        |v: &serde_json::Value| v.get("$$mdtype").and_then(|t| t.as_str()) == Some("Node");
    if is_node(json)
        && let Ok(node) = node_from_json(json)
    {
        return Ok(Revived::Node(Box::new(node)));
    }
    if let Some(items) = json.as_array()
        && !items.is_empty()
        && items.iter().all(is_node)
        && let Ok(nodes) = items
            .iter()
            .map(node_from_json)
            .collect::<std::result::Result<_, _>>()
    {
        return Ok(Revived::Nodes(nodes));
    }
    Scalar::from_json(json)
        .map(Revived::Value)
        .map_err(MarkdocError::ParseError)
}

/// `Ast.isAst(value)`: `!!value?.$$mdtype` (a `Variable`, a `Function`, a tag, or an object
/// with a truthy `$$mdtype`).
pub fn is_ast(value: &Scalar) -> bool {
    crate::next::validator::is_ast(value)
}

/// `Ast.isFunction(value)`: `value?.$$mdtype === 'Function'`.
pub fn is_function(value: &Scalar) -> bool {
    has_mdtype(value, "Function")
}

/// `Ast.isVariable(value)`: `value?.$$mdtype === 'Variable'`.
pub fn is_variable(value: &Scalar) -> bool {
    has_mdtype(value, "Variable")
}

fn has_mdtype(value: &Scalar, name: &str) -> bool {
    match value {
        Scalar::Function(_) => name == "Function",
        Scalar::Variable(_) => name == "Variable",
        Scalar::Tag(_) => name == "Tag",
        Scalar::Object(o) => matches!(o.get("$$mdtype"), Some(Scalar::String(t)) if t == name),
        _ => false,
    }
}

/// How deep node 22 (a fresh process, its default stack, called from the top of a script)
/// walks each shape with `getAstValues` before `RangeError: Maximum call stack size exceeded`:
/// `(shape, fresh, warm)`: arrays in arrays, objects in objects (`work/api/depth/`). The port
/// stops at the fresh limit of each.
pub const NODE_LIMITS: [(&str, usize, usize); 2] =
    [("arrays", 3085, 4480), ("objects", 3216, 4480)];

/// What a level of `getAstValues` costs (see [`crate::next::stack`]): node's stack
/// (`STACK / (limit + 1/2)`) and, where the stack cannot grow, a debug build's.
const COST_ARRAY: (u64, u64) = (342428116, 2 * 1024);
const COST_OBJECT: (u64, u64) = (328481876, 2 * 1024);

/// `Ast.getAstValues(value)`: every AST value in `value`, in the generator's order: an array's
/// items' (each walked), the value itself when it [`is_ast`], and, for a plain object (a
/// [`Scalar::Object`], or a tag read from JSON: `{$$mdtype, name, attributes, children}`), its
/// members' values' (each walked). A value found inside a tag's children (render-tree nodes)
/// is a copy; the rest are borrowed. `Err` is upstream's `RangeError` where node's stack would
/// run out.
pub fn get_ast_values(value: &Scalar) -> Result<Vec<Cow<'_, Scalar>>> {
    let mut out = Vec::new();
    walk(value, &mut out)?;
    Ok(out)
}

fn level(cost: (u64, u64)) -> Result<stack::Level> {
    stack::enter(cost.0, cost.1)
        .ok_or_else(|| MarkdocError::TransformError(value::RANGE_ERROR.into()))
}

fn walk<'a>(value: &'a Scalar, out: &mut Vec<Cow<'a, Scalar>>) -> Result<()> {
    let cost = match value {
        Scalar::Array(_) => COST_ARRAY,
        Scalar::Object(_) | Scalar::Tag(_) => COST_OBJECT,
        // a class's instance: yielded, and not walked into
        Scalar::Variable(_) | Scalar::Function(_) => {
            out.push(Cow::Borrowed(value));
            return Ok(());
        }
        // `value == null || typeof value !== 'object'`
        _ => return Ok(()),
    };
    let _level = level(cost)?;
    stack::grow(|| {
        match value {
            Scalar::Array(items) => {
                for item in items {
                    walk(item, out)?;
                }
            }
            Scalar::Object(o) => {
                if is_ast(value) {
                    out.push(Cow::Borrowed(value));
                }
                for v in o.values() {
                    walk(v, out)?;
                }
            }
            Scalar::Tag(tag) => {
                out.push(Cow::Borrowed(value));
                walk_tag_members(tag, out)?;
            }
            _ => {}
        }
        Ok(())
    })
}

/// The `attributes` and `children` of a tag read from JSON, walked as `Object.values` gives
/// them.
fn walk_tag_members<'a>(tag: &'a Tag, out: &mut Vec<Cow<'a, Scalar>>) -> Result<()> {
    // `attributes`: a plain object
    {
        let _level = level(COST_OBJECT)?;
        stack::grow(|| -> Result<()> {
            if tag
                .attributes
                .get("$$mdtype")
                .is_some_and(|t| Value::from_scalar(t).to_boolean())
            {
                out.push(Cow::Owned(Scalar::Object(tag.attributes.clone())));
            }
            for v in tag.attributes.values() {
                walk(v, out)?;
            }
            Ok(())
        })?;
    }
    // `children`: an array of render-tree nodes
    let _level = level(COST_ARRAY)?;
    stack::grow(|| {
        for child in &tag.children {
            match child {
                RenderableTreeNode::Scalar(v) => walk(v, out)?,
                RenderableTreeNode::Tag(t) => {
                    let _level = level(COST_OBJECT)?;
                    out.push(Cow::Owned(Scalar::Tag(t.clone())));
                    stack::grow(|| walk_tag_members(t, out))?;
                }
            }
        }
        Ok(())
    })
}

/// `Ast.resolve(value, config)` (`base.ts`): the value with its variables and function calls
/// resolved, as [`crate::next::transformer::resolve`] does it.
pub fn resolve(value: &Scalar, config: &Config) -> Result<Value> {
    crate::next::transformer::resolve(value, config)
}

#[cfg(test)]
mod tests {
    //! What node gives for the same values (`findings/conformance/work/api/unit_values.cjs`).
    use super::*;
    use crate::ast::{Function, PathSegment, Variable};
    use crate::next::compare::node_json;
    use crate::next::index::{self, Markdoc, thrown};
    use crate::next::parser::ParserArgs;
    use crate::next::renderers::html;
    use crate::next::schema::Variables;
    use crate::types::NodeType;

    #[test]
    fn from_json_revives_nodes_and_values() {
        // node: Ast.fromJSON('{"$$mdtype":"Node","type":"text","attributes":{"content":"x"}}')
        // is a Node whose JSON has every field (defaulted).
        let Revived::Node(node) =
            from_json(r#"{"$$mdtype":"Node","type":"text","attributes":{"content":"x"}}"#).unwrap()
        else {
            panic!("a node")
        };
        assert_eq!(node.node_type, NodeType::Text);
        assert_eq!(
            node_json(&node),
            serde_json::json!({"$$mdtype": "Node", "errors": [], "lines": [], "inline": false,
                "attributes": {"content": "x"}, "children": [], "type": "text",
                "annotations": [], "slots": {}})
        );
        let revived = from_json(r#"[1, {"$$mdtype": "Variable", "path": ["a"]}]"#).unwrap();
        let Revived::Value(Scalar::Array(items)) = &revived else {
            panic!("a list")
        };
        assert!(matches!(&items[1], Scalar::Variable(v) if v.path == [PathSegment::from("a")]));
        let Revived::Nodes(nodes) =
            from_json(r#"[{"$$mdtype": "Node"}, {"$$mdtype": "Node", "type": "hr"}]"#).unwrap()
        else {
            panic!("nodes")
        };
        assert_eq!(nodes[1].node_type, NodeType::Hr);
        // not JSON: a SyntaxError (its message is serde_json's)
        assert!(thrown(&from_json("{").unwrap_err()).starts_with("SyntaxError: "));
    }

    #[test]
    fn get_ast_values_in_the_generators_order() {
        // node: [...getAstValues([1, {a: Variable(x), $$mdtype: 'X', b: [Function(f)]}])] is
        // the object, then the Variable, then the Function.
        let variable = Scalar::Variable(Variable::new(vec![PathSegment::from("x")]));
        let function = Scalar::Function(Function::new("f".into(), Default::default()));
        let object = Scalar::Object(
            [
                ("a".to_string(), variable.clone()),
                ("$$mdtype".to_string(), Scalar::String("X".into())),
                ("b".to_string(), Scalar::Array(vec![function.clone()])),
            ]
            .into(),
        );
        let value = Scalar::Array(vec![Scalar::Number(1.0), object.clone()]);
        let found = get_ast_values(&value).unwrap();
        assert_eq!(found, [&object, &variable, &function].map(Cow::Borrowed));
        // node: a tag read from JSON is a plain object: its children are walked too
        // ([...getAstValues({$$mdtype: 'Tag', name: 'a', attributes: {}, children:
        // [{$$mdtype: 'Variable', path: ['x']}]})] is the tag, then the Variable)
        let tag = Scalar::from_json(&serde_json::json!({"$$mdtype": "Tag", "name": "a",
            "attributes": {}, "children": [{"$$mdtype": "Variable", "path": ["x"]}]}))
        .unwrap();
        let found = get_ast_values(&tag).unwrap();
        assert_eq!(found.len(), 2);
        assert!(matches!(&*found[1], Scalar::Variable(v) if v.path == [PathSegment::from("x")]));
        // node (work/api/ast_values_probe.cjs): a tag's attributes with a `$$mdtype`, then their
        // values, then its children, a tag among them with its own: a, attributes, y, x, b, f
        let tag = Scalar::from_json(&serde_json::json!({"$$mdtype": "Tag", "name": "a",
            "attributes": {"$$mdtype": "X", "k": {"$$mdtype": "Variable", "path": ["y"]}},
            "children": [{"$$mdtype": "Variable", "path": ["x"]}, {"$$mdtype": "Tag", "name": "b",
                "attributes": {}, "children": [{"$$mdtype": "Function", "name": "f",
                "parameters": {}}]}]}))
        .unwrap();
        let found: Vec<serde_json::Value> = get_ast_values(&tag)
            .unwrap()
            .iter()
            .map(|v| v.to_json())
            .collect();
        let names: Vec<&str> = found
            .iter()
            .map(|v| {
                v.get("name")
                    .or(v.get("path").and_then(|p| p.get(0)))
                    .and_then(|n| n.as_str())
                    .unwrap_or("attributes")
            })
            .collect();
        assert_eq!(names, ["a", "attributes", "y", "x", "b", "f"]);
        // node: isAst({$$mdtype: 'X'}), isFunction(Function), isVariable({$$mdtype:
        // 'Variable'}), isAst(null): true, true, true, false
        let variable_object =
            Scalar::Object([("$$mdtype".to_string(), Scalar::String("Variable".into()))].into());
        assert!(is_ast(&object) && is_function(&function) && is_variable(&variable_object));
        assert!(!is_ast(&Scalar::Null));
    }

    #[test]
    fn get_ast_values_stops_where_a_fresh_node_does() {
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(|| {
                let arrays = |n: usize| {
                    let mut x = Scalar::Number(1.0);
                    for _ in 0..n {
                        x = Scalar::Array(vec![x]);
                    }
                    x
                };
                let n = NODE_LIMITS[0].1;
                let (at, past) = (arrays(n), arrays(n + 1));
                if stack::can_grow() {
                    assert!(get_ast_values(&at).is_ok());
                }
                assert_eq!(
                    thrown(&get_ast_values(&past).unwrap_err()),
                    "RangeError: Maximum call stack size exceeded"
                );
                // a deep Scalar drops without recursing too deep for the thread
                std::mem::forget((at, past));
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn the_default_exports_instance() {
        // node: new Markdoc({variables: {v: 'V'}}).transform(parse('{% $v %}')) renders
        // <article><p>V</p></article>.
        let markdoc = Markdoc::new(Config {
            variables: Some(Variables::Value(Value::from_json(
                &serde_json::json!({"v": "V"}),
            ))),
            ..Config::default()
        });
        let ast = markdoc.parse("{% $v %}", &ParserArgs::default()).unwrap();
        let tree = markdoc.transform(&ast).unwrap();
        assert_eq!(html::render(&tree).unwrap(), "<article><p>V</p></article>");
        assert!(markdoc.validate(&ast).unwrap().is_empty());
        // createElement({key: 1}, ...): a name of any value
        let element = index::create_element(
            Value::from_json(&serde_json::json!({"key": 1})),
            Default::default(),
            vec![Value::from("a"), Value::from("b")],
        );
        assert_eq!(
            value::json_stringify(&element, "").unwrap().unwrap(),
            r#"{"name":{"key":1},"attributes":{},"children":["a","b"]}"#
        );
    }
}
