//! Control flow beyond upstream's `if`/`else`: `{% switch %}` with `{% case %}`, and
//! `{% for %}` (an extension, off unless its schemas are put in the config).
//!
//! Upstream Markdoc has no loops in documents ("Markdoc does not support writing loops
//! directly in documents": its loop is a custom tag with a `transform` of its own). These are
//! such tags: [`schemas`] gives their [`Schema`]s, whose `transform` closures do the work at
//! transform time, as upstream's own `if` and `partial` do. Without them in `config.tags` a
//! document renders as upstream renders it: `switch`, `case` and `for` are undeclared tags,
//! which render their children once (and `validate` reports `tag-undefined`).
//!
//! - `{% switch $x %}` renders the children of its first `{% case %}` whose value is `===`
//!   the subject (upstream's `equals`), or of the first `{% case %}` without a value (a
//!   default; order decides, as with `{% else /%}`); nothing if none matches. Cases are
//!   found among the switch's children and inside the paragraphs and other non-tag nodes
//!   that hold them (`{% case 1 %}one{% /case %}` written on one line is inside a
//!   paragraph), never inside another tag, so a nested switch keeps its own cases. A case
//!   outside a switch renders its children.
//! - `{% for $items as="item" index="index" %}` renders its children once per element of the
//!   array, with `$item` and `$index` (0-based) bound for the body (the names default to
//!   `item` and `index`; a nested loop that reuses a name shadows the outer one). An
//!   undefined or `null` subject renders nothing. A subject of another kind, and `as` or
//!   `index` that are not literal non-empty strings, are an `Err` (crate behaviour: upstream
//!   never fails on a value). When the body begins and ends with a list of the same kind
//!   (`ordered`), the list one iteration ends with and the list the next begins with are one
//!   list, the first's attributes kept: a body that is one bullet makes one list.
//!
//! [`apply`] puts the schemas in a config, completing any the config already declares (with
//! a description, attributes or validation of its own); [`apply_bundle`] does so with the
//! declarations a schema bundle keeps for this layer (`layer = "control_flow"`, which
//! [`Bundle::config`] leaves out).
//!
//! [`evaluate_conditionals`] does the same, and evaluates `if`/`else` too, on the AST, for
//! callers that run AST passes before `transform`; it resolves values with
//! [`crate::next::transformer::resolve`] and decides with upstream's truthiness (anything but
//! `false`, `undefined` and `null`).
//!
//! ```
//! use markdoc::ext::control_flow;
//! use markdoc::next::schema::{Config, Variables, value::Value};
//! use markdoc::next::{index, parser::ParserArgs, renderers::html};
//!
//! let doc = index::parse(
//!     "{% for $robots as=\"robot\" %}\n- {% $robot %}\n{% /for %}\n\n\
//!      {% switch $count %}{% case 2 %}two{% /case %}{% case %}other{% /case %}{% /switch %}\n",
//!     &ParserArgs::default(),
//! )
//! .unwrap();
//! let mut config = Config::default();
//! control_flow::apply(&mut config);
//! config.variables = Some(Variables::Value(Value::from_json(&serde_json::json!({
//!     "robots": ["Bot100", "Bot250"],
//!     "count": 2
//! }))));
//! let tree = index::transform(&doc, &config).unwrap();
//! assert_eq!(
//!     html::render(&tree).unwrap(),
//!     "<article><ul><li>Bot100</li><li>Bot250</li></ul><p>two</p></article>"
//! );
//! ```

use crate::ast::{Node, PathSegment, Variable};
use crate::ext::bundle::Bundle;
use crate::next::index::merge_config;
use crate::next::schema::value::{self, Value, strict_equals};
use crate::next::schema::{Config, Schema, SchemaAttribute, TransformFn, Variables};
use crate::next::tags::conditional::truthy;
use crate::next::transformer::{ResolvedNode, flat_push, resolve, resolve_node, resolve_variable};
use crate::types::{JsObject, MarkdocError, NodeType, Result, Scalar};
use indexmap::IndexMap;
use std::sync::Arc;

/// How deeply `for` loops may nest directly in each other's bodies in
/// [`evaluate_conditionals`] (each level a recursion there); one more is an `Err`.
pub const MAX_LOOP_NESTING: usize = 200;

fn error(message: impl Into<String>) -> MarkdocError {
    MarkdocError::TransformError(message.into())
}

fn is_tag(node: &Node, name: &str) -> bool {
    node.node_type == NodeType::Tag && node.tag.as_deref() == Some(name)
}

fn attribute(name: &str, attribute: SchemaAttribute) -> (String, SchemaAttribute) {
    (
        name.to_string(),
        SchemaAttribute {
            render: Some(Value::Boolean(false)),
            ..attribute
        },
    )
}

/// The `switch` tag: `primary` (required) is the subject.
pub fn switch() -> Schema {
    let transform: TransformFn = Arc::new(transform_switch);
    Schema {
        attributes: Some(IndexMap::from([attribute(
            "primary",
            SchemaAttribute {
                required: true,
                ..SchemaAttribute::default()
            },
        )])),
        transform: Some(transform),
        description: Some("Select one of several bodies by matching a value".into()),
        ..Schema::default()
    }
}

/// The `case` tag: `primary` is the value it matches; without one it is the default. It has
/// no transform of its own: its switch transforms the children of the one that matches.
pub fn case() -> Schema {
    Schema {
        attributes: Some(IndexMap::from([attribute(
            "primary",
            SchemaAttribute::default(),
        )])),
        description: Some("One branch of a `{% switch %}`; no value makes it the default".into()),
        ..Schema::default()
    }
}

/// The `for` tag: `primary` (required) is the array, `as` and `index` the names bound.
pub fn for_loop() -> Schema {
    let transform: TransformFn = Arc::new(transform_for);
    let string = || SchemaAttribute {
        attr_type: Some(crate::next::schema::TypeSpec::Builtin(
            crate::next::schema::BuiltinType::String,
        )),
        ..SchemaAttribute::default()
    };
    Schema {
        attributes: Some(IndexMap::from([
            attribute(
                "primary",
                SchemaAttribute {
                    required: true,
                    ..SchemaAttribute::default()
                },
            ),
            attribute("as", string()),
            attribute("index", string()),
        ])),
        transform: Some(transform),
        description: Some("Repeat the body once per element of an array".into()),
        ..Schema::default()
    }
}

/// `switch`, `case` and `for`, for `config.tags`.
pub fn schemas() -> IndexMap<String, Option<Schema>> {
    IndexMap::from([
        ("switch".to_string(), Some(switch())),
        ("case".to_string(), Some(case())),
        ("for".to_string(), Some(for_loop())),
    ])
}

/// The layer's name in a schema bundle's data (`layer = "control_flow"` in `tags.toml`).
pub const LAYER: &str = "control_flow";

/// Puts the schemas `bundle` declares for this layer's tags (their descriptions, attributes
/// and validation; [`Bundle::config`] leaves them out) into `config`, and completes them with
/// [`apply`]. A tag `config` hides (`None`) stays hidden.
pub fn apply_bundle(bundle: &Bundle, config: &mut Config) {
    for (name, schema) in bundle.layer_schemas(LAYER) {
        if !matches!(config.tags.get(&name), Some(None)) {
            config.tags.insert(name, Some(schema));
        }
    }
    apply(config);
}

/// Puts [`schemas`] in `config.tags`. A schema the config already has for one of the names
/// (a schema bundle's, with its descriptions and validation) keeps what it says and gets the
/// `transform`, and the attributes it does not declare; an entry that hides the name
/// (`None`) is left alone.
pub fn apply(config: &mut Config) {
    for (name, schema) in schemas() {
        let schema = schema.expect("schemas() has only schemas");
        match config.tags.get_mut(&name) {
            Some(Some(existing)) => {
                existing.transform = schema.transform;
                let attributes = existing.attributes.get_or_insert_with(IndexMap::new);
                for (key, attribute) in schema.attributes.into_iter().flatten() {
                    attributes.entry(key).or_insert(attribute);
                }
            }
            Some(None) => {}
            None => {
                config.tags.insert(name, Some(schema));
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// At transform time
// ---------------------------------------------------------------------------------------

/// The `case` nodes of a switch: its case children, and the cases inside the non-tag nodes
/// among them (paragraphs, inlines), in document order, never inside another tag.
fn cases_of<'n, 'a>(switch: &'n ResolvedNode<'a>) -> Vec<&'n ResolvedNode<'a>> {
    let mut found = Vec::new();
    let mut stack: Vec<&ResolvedNode<'_>> = switch.children.iter().rev().collect();
    while let Some(node) = stack.pop() {
        if is_tag(node.node, "case") {
            found.push(node);
        } else if node.node.node_type != NodeType::Tag {
            stack.extend(node.children.iter().rev());
        }
    }
    found
}

fn transform_switch(node: &ResolvedNode<'_>, config: &Config) -> Result<Value> {
    // `node.attributes.primary`, read as upstream reads attributes (through their prototype).
    let subject = node.attribute("primary")?;
    let mut chosen = None;
    for case in cases_of(node) {
        // A case without a primary matches anything.
        let matched = match &case.attributes {
            Value::Object(o) if !o.contains_key("primary") && o.proto().is_object_prototype() => {
                true
            }
            _ => {
                let value = case.attribute("primary")?;
                value.is_undefined() || strict_equals(&subject, &value)
            }
        };
        if matched {
            chosen = Some(case);
            break;
        }
    }
    let mut out = Vec::new();
    for child in chosen.map(|case| &case.children[..]).unwrap_or_default() {
        flat_push(&mut out, child.transform(config)?);
    }
    Ok(Value::array(out))
}

/// What a value is, for messages.
fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Undefined => "undefined",
        Value::Null => "null",
        Value::Boolean(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
        Value::Tag(_) => "a tag",
        // A built-in object or function a document reached (`$x.constructor`).
        Value::Builtin(_) => "a built-in",
    }
}

/// The name a `for` binds for `key` (`as` or `index`), `fallback` when it is not given: a
/// literal, non-empty string written in the document.
fn binding_name(node: &Node, key: &str, fallback: &str) -> Result<String> {
    match node.attributes.get(key) {
        None => Ok(fallback.to_string()),
        Some(Scalar::String(s)) if !s.is_empty() => Ok(s.clone()),
        Some(Scalar::String(_)) => Err(error(format!(
            "a `for` tag's `{key}` is empty, and a variable needs a name."
        ))),
        Some(Scalar::Variable(_) | Scalar::Function(_)) => Err(error(format!(
            "a `for` tag's `{key}` is the *name* of the variable the loop binds, so it must \
             be a literal string, not an expression, which would make the name depend on the \
             data it is naming."
        ))),
        Some(other) => Err(error(format!(
            "a `for` tag's `{key}` must be a string naming a variable, not {}.",
            kind_of(&Value::from_scalar(other))
        ))),
    }
}

/// The elements a `for` iterates: none for `undefined` and `null`, an `Err` for anything
/// but an array.
fn elements(subject: &Value) -> Result<Vec<Value>> {
    match subject {
        Value::Undefined | Value::Null => Ok(Vec::new()),
        // Every index, a hole as `undefined`.
        Value::Array(items) => Ok((0..items.len())
            .map(|i| items.get(i).cloned().unwrap_or(Value::Undefined))
            .collect()),
        other => Err(error(format!(
            "a `for` tag needs an array to iterate and was given {}. An unset variable is \
             undefined and renders nothing; this is a value of the wrong kind.",
            kind_of(other)
        ))),
    }
}

/// The config one iteration runs under: the config's variables with the two names bound.
fn scoped(config: &Config, names: &[String; 2], item: Value, index: usize) -> Config {
    let bound = [item, Value::Number(index as f64)];
    let variables = match &config.variables {
        Some(Variables::Fn(outer)) => {
            let (outer, names, bound) = (outer.clone(), names.clone(), bound.clone());
            Variables::Fn(Arc::new(move |path: &[PathSegment]| {
                let own = match path.first() {
                    // The later name wins when both are the same, as in an object.
                    Some(PathSegment::String(head)) => names.iter().rposition(|n| n == head),
                    _ => None,
                };
                let Some(at) = own else {
                    return outer(path);
                };
                let mut value = bound[at].clone();
                for step in &path[1..] {
                    if value.is_undefined() {
                        value = Value::empty_object();
                    }
                    value = value::get_property(&value, &value::property_key(step))
                        .unwrap_or(Value::Undefined);
                }
                value
            }))
        }
        other => {
            let mut object = JsObject::new();
            if let Some(Variables::Value(outer)) = other {
                object.extend(value::own_entries(outer));
            }
            for (name, value) in names.iter().zip(bound) {
                object.insert(name.clone(), value);
            }
            Variables::Value(Value::object(object))
        }
    };
    Config {
        variables: Some(variables),
        ..config.clone()
    }
}

/// Is the body one whose seams join lists: it begins and ends with lists of the same kind?
fn joins_lists(body: &[Node]) -> bool {
    match (body.first(), body.last()) {
        (Some(first), Some(last)) => {
            first.node_type == NodeType::List
                && last.node_type == NodeType::List
                && first.attributes.get("ordered") == last.attributes.get("ordered")
        }
        _ => false,
    }
}

fn transform_for(node: &ResolvedNode<'_>, config: &Config) -> Result<Value> {
    let names = [
        binding_name(node.node, "as", "item")?,
        binding_name(node.node, "index", "index")?,
    ];
    let items = elements(&node.attribute("primary")?)?;
    let body = &node.node.children;
    let joins = joins_lists(body);
    let mut out: Vec<Value> = Vec::new();
    for (index, item) in items.into_iter().enumerate() {
        let config = scoped(config, &names, item, index);
        let mut iteration = Vec::new();
        for child in body {
            let resolved = resolve_node(child, &config)?;
            flat_push(&mut iteration, resolved.transform(&config)?);
        }
        let mut iteration = iteration.into_iter();
        let Some(first) = iteration.next() else {
            continue;
        };
        match (out.last_mut(), first) {
            (Some(Value::Tag(previous)), Value::Tag(next))
                if joins && strict_equals(&previous.name, &next.name) =>
            {
                Arc::make_mut(previous)
                    .children
                    .extend(next.children.iter().cloned());
            }
            (_, first) => out.push(first),
        }
        out.extend(iteration);
    }
    Ok(Value::array(out))
}

// ---------------------------------------------------------------------------------------
// On the AST
// ---------------------------------------------------------------------------------------

/// `node` with every `if`, `switch` and `for` replaced by what it renders: the children of
/// the branch, the case, or one copy of the body per element (each copy with the loop's
/// variables replaced by their values, wherever an attribute or a text's content reads them,
/// in function calls, arrays and objects too; every other variable is left for `transform`),
/// with lists joined at the seams as the `for` tag joins them. Conditions and subjects are
/// resolved with `config` (its variables, and its functions over upstream's six) and decided
/// with upstream's truthiness. When `node` itself is one of these tags and renders to other
/// than one node, the result is a document holding them.
///
/// `Err` as the `for` tag's, and past [`MAX_LOOP_NESTING`] loops directly inside each
/// other; any other depth (no recursion).
pub fn evaluate_conditionals(node: &Node, config: &Config) -> Result<Node> {
    let config = merge_config(config);
    let mut top = splice(vec![node.clone()], &config, 0)?;
    let mut root = if top.len() == 1 {
        top.pop().expect("one node")
    } else {
        Node::new(NodeType::Document, std::iter::empty(), top, None)
    };
    let mut stack: Vec<&mut Node> = vec![&mut root];
    while let Some(node) = stack.pop() {
        let children = std::mem::take(&mut node.children);
        node.children = splice(children, &config, 0)?;
        let Node {
            children, slots, ..
        } = node;
        stack.extend(slots.values_mut());
        stack.extend(children.iter_mut());
    }
    Ok(root)
}

/// `nodes` with every `if`, `switch` and `for` among them (not inside them) replaced by what
/// it renders, until none is left among them.
fn splice(nodes: Vec<Node>, config: &Config, loops: usize) -> Result<Vec<Node>> {
    let mut out = Vec::with_capacity(nodes.len());
    let mut queue = nodes;
    queue.reverse();
    while let Some(node) = queue.pop() {
        if is_tag(&node, "if") {
            queue.extend(chosen_branch(&node, config)?.into_iter().rev());
        } else if is_tag(&node, "switch") {
            queue.extend(chosen_case(&node, config)?.into_iter().rev());
        } else if is_tag(&node, "for") {
            if loops >= MAX_LOOP_NESTING {
                return Err(error(format!(
                    "`for` loops nested more than {MAX_LOOP_NESTING} deep"
                )));
            }
            let joins = joins_lists(&node.children);
            let mut iterations = Vec::new();
            for iteration in iterations_of(&node, config)? {
                append_iteration(
                    &mut iterations,
                    splice(iteration, config, loops + 1)?,
                    joins,
                );
            }
            out.extend(iterations);
        } else {
            out.push(node);
        }
    }
    Ok(out)
}

fn primary(node: &Node, config: &Config) -> Result<Value> {
    match node.attributes.get("primary") {
        Some(value) => resolve(value, config),
        None => Ok(Value::Undefined),
    }
}

/// Upstream's `if`: the children up to the first `else` when its condition is truthy, else
/// those after the first `else` whose condition is (true without one).
fn chosen_branch(node: &Node, config: &Config) -> Result<Vec<Node>> {
    let mut taken = truthy(&primary(node, config)?);
    let mut out = Vec::new();
    for child in &node.children {
        if is_tag(child, "else") {
            if taken {
                break;
            }
            taken = match child.attributes.get("primary") {
                Some(value) => truthy(&resolve(value, config)?),
                None => true,
            };
        } else if taken {
            out.push(child.clone());
        }
    }
    Ok(out)
}

fn chosen_case(node: &Node, config: &Config) -> Result<Vec<Node>> {
    let subject = primary(node, config)?;
    let mut stack: Vec<&Node> = node.children.iter().rev().collect();
    while let Some(child) = stack.pop() {
        if is_tag(child, "case") {
            let matched = match child.attributes.get("primary") {
                None => true,
                Some(value) => strict_equals(&subject, &resolve(value, config)?),
            };
            if matched {
                return Ok(child.children.clone());
            }
        } else if child.node_type != NodeType::Tag {
            stack.extend(child.children.iter().rev());
        }
    }
    Ok(Vec::new())
}

/// A `for`'s body once per element, the loop's variables bound in each copy.
fn iterations_of(node: &Node, config: &Config) -> Result<Vec<Vec<Node>>> {
    let names = [
        binding_name(node, "as", "item")?,
        binding_name(node, "index", "index")?,
    ];
    let items = elements(&primary(node, config)?)?;
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.into_iter().enumerate() {
        // Only the loop's own names: what else a variable reads is left for `transform`.
        let bindings = scoped(&Config::default(), &names, item, index);
        let mut body = node.children.clone();
        bind(&mut body, &names, &bindings)?;
        out.push(body);
    }
    Ok(out)
}

/// Replaces, in `body`, every variable that reads one of `names` with its value under
/// `bindings`; a nested `for` that binds a name again owns it below its own attributes.
fn bind(body: &mut [Node], names: &[String; 2], bindings: &Config) -> Result<()> {
    let mut stack: Vec<(&mut Node, [bool; 2])> =
        body.iter_mut().map(|node| (node, [true, true])).collect();
    while let Some((node, active)) = stack.pop() {
        for value in node.attributes.values_mut() {
            substitute(value, names, active, bindings)?;
        }
        let mut inner = active;
        if is_tag(node, "for") {
            let own = [
                binding_name(node, "as", "item")?,
                binding_name(node, "index", "index")?,
            ];
            for (k, name) in names.iter().enumerate() {
                if own.contains(name) {
                    inner[k] = false;
                }
            }
        }
        if inner == [false, false] {
            continue;
        }
        let Node {
            children, slots, ..
        } = node;
        stack.extend(slots.values_mut().map(|slot| (slot, inner)));
        stack.extend(children.iter_mut().map(|child| (child, inner)));
    }
    Ok(())
}

/// [`bind`] in one value (a value nests no deeper than the tag grammar's limit).
fn substitute(
    value: &mut Scalar,
    names: &[String; 2],
    active: [bool; 2],
    bindings: &Config,
) -> Result<()> {
    match value {
        Scalar::Variable(variable) if reads(variable, names, active) => {
            *value = match resolve_variable(variable, bindings)? {
                Value::Undefined => Scalar::Undefined,
                resolved => resolved.to_scalar().map_err(error)?,
            };
        }
        Scalar::Function(function) => {
            for parameter in function.parameters.values_mut() {
                substitute(parameter, names, active, bindings)?;
            }
        }
        Scalar::Array(items) => {
            for item in items {
                substitute(item, names, active, bindings)?;
            }
        }
        Scalar::Object(object) => {
            for item in object.values_mut() {
                substitute(item, names, active, bindings)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn reads(variable: &Variable, names: &[String; 2], active: [bool; 2]) -> bool {
    match variable.path.first() {
        Some(PathSegment::String(head)) => names
            .iter()
            .zip(active)
            .any(|(name, on)| on && name == head),
        _ => false,
    }
}

/// One iteration's nodes after the others: the list it begins with continues the list the
/// last one ended with, when the body joins lists and both are lists of the same kind.
fn append_iteration(out: &mut Vec<Node>, iteration: Vec<Node>, joins: bool) {
    let mut iteration = iteration.into_iter();
    let Some(mut first) = iteration.next() else {
        return;
    };
    match out.last_mut() {
        Some(previous) if joins && continues_a_list(previous, &first) => {
            previous.children.append(&mut first.children);
        }
        _ => out.push(first),
    }
    out.extend(iteration);
}

fn continues_a_list(previous: &Node, next: &Node) -> bool {
    previous.node_type == NodeType::List
        && next.node_type == NodeType::List
        && previous.attributes.get("ordered") == next.attributes.get("ordered")
}

#[cfg(test)]
mod tests;
