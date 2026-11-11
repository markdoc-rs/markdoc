//! Port of upstream Markdoc v0.5.11's `src/validator.ts`.
//!
//! Owner: the semantics agent.
//!
//! `validator`, `validateTree`, `validateType`, `walkWithParents`.
//!
//! The validator reads the AST as parsed (attribute values are [`Scalar`]s, variables and
//! function calls unresolved), as upstream's does, and reads it as JavaScript does: a value
//! the AST holds as [`Scalar::Js`] is an AST value, a variable or a call by its `$$mdtype`
//! read through its prototype chain (`Ast.isAst`, `isFunction`, `isVariable`), a variable's
//! path by JavaScript's iteration (`for (const key of value.path)`) and a call's parameters by
//! `Object.entries`; `value.constructor === type` finds the constructor through the chain; a
//! required attribute or slot is missing only when the attributes' or slots' prototype (a
//! `__proto__` attribute's value, a slot named `__proto__`) does not have it either; a
//! schema that a config read from JSON gave as `null` throws where upstream reads its fields.
//! Where upstream's code throws (a variable's path through `null` when `config.variables` is
//! set, a type named after a built-in method), the functions here return `Err`.
//!
//! Upstream's promise handling (a schema's `validate` returning a promise) has no counterpart.
//!
//! `walkWithParents` keeps one path from the root while it walks, and the walk keeps its own
//! stack: memory is linear in the depth (upstream copies the ancestors for every node).
//!
//! Each entry of `validate()`'s output is upstream's `ValidateError` ([`ValidateError`]):
//! `{type, lines, location, error}`, placed by the error's own `location` when both its ends
//! have a numeric `line` (a tag in a fence, a hand-made location: `lines` those two lines,
//! `location` `{file: node.location?.file, ...error.location}`), else by its node (the node's
//! `lines` and `location`); and this crate's [`ValidateError::span`] (with the parser's
//! [`crate::parser::ParserArgs::spans`] on; otherwise `None`). An error that is `null`
//! (a hand-made token's) throws `TypeError: Cannot read properties of null (reading
//! 'location')`, as upstream's `error.location` does.
//!
//! # Divergences from upstream
//!
//! - Depth: `validate` throws upstream's `RangeError` for a tree whose deepest node is the
//!   4,047th on its path from the root ([`MAX_VALIDATE_DEPTH`]), where node 22 throws when
//!   `validate` is called from the top of a script, fresh or warmed up. Node's limit moves with
//!   the stack below the call (4,024 from 50 frames deeper); this port's moves with what the
//!   calls below it charge the depth budget ([`crate::depth`]: a `validate` called from a
//!   schema's transform in Rust takes less), not with a caller's own frames
//!   (`tests::validate_throws_past_upstreams_depth`).

use crate::ast::{Function, Node};
use crate::next::schema::value::{self, BuiltinProto, Ctor, Value, realm, scalar_to_js_string};
use crate::next::schema::{
    self, BuiltinType, Config, Found, Matches, SchemaAttribute, TypeSpec, ValidationContext,
    Variables, is_undefined_in, lookup,
};
use crate::next::transformer::find_schema;
use crate::types::{
    JsObjectValue, Location, MarkdocError, NodeType, Result, Scalar, Span, ValidationError,
    ValidationLevel,
};
use indexmap::IndexMap;
use serde_json::Value as Json;

/// One entry of `validate()`'s output: upstream's `ValidateError`, `{type, lines, location,
/// error}` (in that order in its JSON), and this crate's [`Span`] of the error.
#[derive(Debug, Clone)]
pub struct ValidateError {
    /// The node's type (`type`).
    pub node_type: NodeType,
    /// `lines`: the node's [`Node::lines`], or `[location.start.line, location.end.line]`
    /// when the error's own location places the entry (JS numbers: a hand-made token's error
    /// location may hold any).
    pub lines: Vec<f64>,
    /// `location` (`None`, which JSON leaves out, when there is none): see
    /// [`ValidateLocation`].
    pub location: Option<ValidateLocation>,
    pub error: ValidationError,
    /// This crate's position of the error: its own [`ValidationError::span`], else its
    /// node's [`Node::span`]; `None` unless the AST was parsed with
    /// [`crate::parser::ParserArgs::spans`] on.
    pub span: Option<Span>,
}

/// The `location` of a [`ValidateError`], as upstream's `toValidateError` makes it.
#[derive(Debug, Clone, PartialEq)]
pub enum ValidateLocation {
    /// `node.location`: the entry is placed by its node.
    Node(Location),
    /// `{file: node.location?.file, ...error.location}`: the entry is placed by the error's
    /// own location, both of whose ends have a numeric `line` (a tag in a fence, a row of a
    /// table, a hand-made token's), as JSON: the spread copies whatever the error's location
    /// holds, keys in a JS object's order.
    Error(Json),
}

impl ValidateLocation {
    /// The location as upstream's JSON writes it.
    pub fn to_json(&self) -> Json {
        match self {
            ValidateLocation::Node(location) => location.to_json(),
            ValidateLocation::Error(location) => location.clone(),
        }
    }
}

/// `validateType`'s result: `true`/`false`, or the errors a type's own `validate` returned.
#[derive(Debug, Clone)]
pub enum Valid {
    Bool(bool),
    Errors(Vec<ValidationError>),
}

impl Valid {
    /// The result's truthiness (`type.some(...)` reads it so): an array of errors, even an
    /// empty one, is truthy.
    fn truthy(&self) -> bool {
        match self {
            Valid::Bool(b) => *b,
            Valid::Errors(_) => true,
        }
    }
}

fn error(id: &str, level: ValidationLevel, message: String) -> ValidationError {
    ValidationError {
        id: id.into(),
        level,
        message,
        location: None,
        raw: None,
        span: None,
    }
}

fn throw(message: impl Into<String>) -> MarkdocError {
    MarkdocError::ValidationError(message.into())
}

/// `Ast.isAst(value)`: a `Variable`, a `Function`, a tag, or an object with a truthy
/// `$$mdtype` (own or, for a value the plain variants do not hold, inherited).
pub fn is_ast(value: &Scalar) -> bool {
    match value {
        Scalar::Variable(_) | Scalar::Function(_) | Scalar::Tag(_) => true,
        Scalar::Object(o) => o
            .get("$$mdtype")
            .is_some_and(|t| Value::from_scalar(t).to_boolean()),
        Scalar::Js(js) => value::is_ast(js.value()),
        _ => false,
    }
}

/// `Ast.isFunction(value)`: `value?.$$mdtype === 'Function'`.
fn is_function(value: &Scalar) -> bool {
    match value {
        Scalar::Function(_) => true,
        Scalar::Js(js) => value::mdtype_is(js.value(), "Function"),
        _ => false,
    }
}

/// `Ast.isVariable(value)`: `value?.$$mdtype === 'Variable'`.
fn is_variable(value: &Scalar) -> bool {
    match value {
        Scalar::Variable(_) => true,
        Scalar::Js(js) => value::mdtype_is(js.value(), "Variable"),
        _ => false,
    }
}

/// A value as the AST's [`Scalar`] holds it: a primitive as itself, an object as
/// [`Scalar::Js`] (no copy). (A string here is always well formed: it comes from the AST.)
fn scalar_of(value: &Value) -> Scalar {
    match value {
        Value::Undefined => Scalar::Undefined,
        Value::Null => Scalar::Null,
        Value::Boolean(b) => Scalar::Boolean(*b),
        Value::Number(n) => Scalar::Number(*n),
        Value::String(s) => Scalar::String(s.to_string_lossy().into_owned()),
        object => Scalar::Js(JsObjectValue::new(object.clone())),
    }
}

/// `fn.name`: a call's name as a key (`config.functions[fn.name]`).
fn function_name(value: &Scalar) -> Result<String> {
    match value {
        Scalar::Function(f) => Ok(f.name.clone()),
        Scalar::Js(js) => {
            let name = value::get(js.value(), "name").map_err(throw)?;
            value::to_property_key(&name).map_err(throw)
        }
        _ => unreachable!("a call"),
    }
}

/// What `TypeMappings[name]` gives for a type named by a string.
enum Mapped {
    Builtin(BuiltinType),
    /// `Object.prototype` (for `'__proto__'`): an object, not a constructor.
    Prototype,
    /// A method of `Object.prototype`: `new` of it throws.
    NotConstructor,
    /// `undefined`.
    Undefined,
}

fn type_mapping(name: &str) -> Mapped {
    if let Some(builtin) = BuiltinType::from_name(name) {
        return Mapped::Builtin(builtin);
    }
    match name {
        // TypeMappings.constructor is Object.
        "constructor" => Mapped::Builtin(BuiltinType::Object),
        "__proto__" => Mapped::Prototype,
        _ if value::is_object_prototype_name(name) => Mapped::NotConstructor,
        _ => Mapped::Undefined,
    }
}

/// `value.constructor === ctor` for an AST value that is not `null` (and not an AST object).
fn has_constructor(value: &Scalar, ctor: BuiltinType) -> bool {
    if let Scalar::Js(js) = value {
        return value::constructor_is(
            js.value(),
            match ctor {
                BuiltinType::String => Ctor::String,
                BuiltinType::Number => Ctor::Number,
                BuiltinType::Boolean => Ctor::Boolean,
                BuiltinType::Object => Ctor::Object,
                BuiltinType::Array => Ctor::Array,
            },
        );
    }
    match value {
        Scalar::String(_) => ctor == BuiltinType::String,
        Scalar::Number(_) => ctor == BuiltinType::Number,
        Scalar::Boolean(_) => ctor == BuiltinType::Boolean,
        Scalar::Array(_) => ctor == BuiltinType::Array,
        // An own `constructor` property is a JSON value, never a constructor.
        Scalar::Object(o) => !o.contains_key("constructor") && ctor == BuiltinType::Object,
        Scalar::Undefined
        | Scalar::Null
        | Scalar::Variable(_)
        | Scalar::Function(_)
        | Scalar::Tag(_)
        | Scalar::Js(_) => false,
    }
}

/// `validateType(type, value, config, key)`.
pub fn validate_type(
    t: &TypeSpec,
    value: &Scalar,
    cx: &ValidationContext<'_>,
    key: &str,
) -> Result<Valid> {
    if matches!(t, TypeSpec::Falsy) {
        return Ok(Valid::Bool(true));
    }

    if is_function(value) && cx.config.validation.validate_functions {
        let valid = match lookup(&cx.config.functions, &function_name(value)?) {
            Found::Own(Some(schema)) => match &schema.returns {
                None => true,
                Some(returns) => returns.iter().any(|r| r.identical(t)),
            },
            // `!schema?.returns`: no schema, or a built-in function without `returns`.
            _ => true,
        };
        return Ok(Valid::Bool(valid));
    }

    if is_ast(value) {
        return Ok(Valid::Bool(true));
    }

    if let TypeSpec::Union(types) = t {
        for t in types {
            if validate_type(t, value, cx, key)?.truthy() {
                return Ok(Valid::Bool(true));
            }
        }
        return Ok(Valid::Bool(false));
    }

    let ctor = match t {
        TypeSpec::Name(name) => match type_mapping(name) {
            Mapped::Builtin(b) => Some(b),
            // `new type()`: V8 names the callee by its source text, the variable `type`.
            Mapped::NotConstructor => {
                return Err(throw("TypeError: type is not a constructor"));
            }
            Mapped::Prototype | Mapped::Undefined => None,
        },
        TypeSpec::Builtin(b) => Some(*b),
        TypeSpec::Markdoc(m) => {
            return Ok(Valid::Errors(
                m.validate(value, cx.config, key).map_err(throw)?,
            ));
        }
        TypeSpec::Custom(custom) => {
            if let Some(errors) = custom.validate(value, cx, key) {
                return Ok(Valid::Errors(errors?));
            }
            None
        }
        // `new type()` of a function that is no constructor throws; `Object.prototype` is no
        // function, and no value's `constructor` is it.
        TypeSpec::Realm(b) if b.is_callable() && !b.is_constructor() => {
            return Err(throw("TypeError: type is not a constructor"));
        }
        TypeSpec::Realm(_) => None,
        // An object or other value that no value's `constructor` is.
        TypeSpec::Value(_) => None,
        TypeSpec::Falsy | TypeSpec::Union(_) => unreachable!("handled above"),
    };

    Ok(Valid::Bool(
        !matches!(value, Scalar::Undefined | Scalar::Null)
            && ctor.is_some_and(|c| has_constructor(value, c)),
    ))
}

/// `typeToString(type)`.
pub fn type_to_string(t: &TypeSpec) -> String {
    match t {
        TypeSpec::Name(name) => name.clone(),
        TypeSpec::Union(types) => types
            .iter()
            .map(type_to_string)
            .collect::<Vec<_>>()
            .join(" | "),
        TypeSpec::Builtin(b) => b.name().into(),
        TypeSpec::Markdoc(m) => m.name().into(),
        TypeSpec::Custom(c) => c.name().into(),
        // `type.name`: a function's own `name`; `Object.prototype` has none (`undefined`).
        TypeSpec::Realm(b) => match value::get_property(&Value::Builtin(*b), "name") {
            Ok(name) => name
                .to_js_string()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Err(_) => String::new(),
        },
        // Never asked for: a falsy type validates every value.
        TypeSpec::Falsy => String::new(),
        // `type.name`.
        TypeSpec::Value(v) => value::get(v, "name")
            .and_then(|n| n.to_js_string())
            .map(value::JsString::into_string_lossy)
            .unwrap_or_else(|e| e),
    }
}

/// `validateFunction(fn, config)`: `fn` a call (a [`Scalar::Function`], or any value whose
/// `$$mdtype` is `'Function'`: its name and parameters read through its chain).
fn validate_function(f: &Scalar, cx: &ValidationContext<'_>) -> Result<Vec<ValidationError>> {
    let name = function_name(f)?;
    let schema = match lookup(&cx.config.functions, &name) {
        Found::Own(Some(schema)) => schema,
        Found::Own(None) | Found::Missing => {
            return Ok(vec![error(
                "function-undefined",
                ValidationLevel::Critical,
                format!(
                    "Undefined function: '{}'",
                    scalar_to_js_string(&name_value(f)?).map_err(throw)?
                ),
            )]);
        }
        // A built-in function of every object: no `validate`, no `parameters`.
        Found::Inherited => return Ok(Vec::new()),
    };
    let mut errors = Vec::new();

    if let Some(validate) = &schema.validate {
        errors.extend(validate(&as_function(f, &name), cx)?);
    }

    // `fn.parameters`: the call's own (a plain object), or what its chain gives.
    let parameters: Option<Value> = match f {
        Scalar::Function(_) => None,
        Scalar::Js(js) => Some(value::get(js.value(), "parameters").map_err(throw)?),
        _ => unreachable!("a call"),
    };

    if let Some(schema_parameters) = &schema.parameters {
        // `Object.entries(fn.parameters)`.
        let entries: Vec<(String, Scalar)> = match (f, &parameters) {
            (Scalar::Function(plain), _) => plain
                .parameters
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            (_, Some(p)) => value::own_entries_of(p)
                .map_err(throw)?
                .into_iter()
                .map(|(k, v)| (k, scalar_of(&v)))
                .collect(),
            _ => unreachable!("a call"),
        };
        for (key, value) in &entries {
            // `const param = schema.parameters?.[key]; if (!param) ...`.
            let param = match lookup(schema_parameters, key) {
                Found::Own(param) if !param.kind.is_falsy() => param,
                // Inherited: a function, with no `type`.
                Found::Inherited => continue,
                Found::Own(_) | Found::Missing => {
                    errors.push(error(
                        "parameter-undefined",
                        ValidationLevel::Error,
                        format!("Invalid parameter: '{key}'"),
                    ));
                    continue;
                }
            };

            if is_ast(value) && !is_function(value) {
                continue;
            }

            if let Some(t) = &param.attr_type {
                match validate_type(t, value, cx, key)? {
                    Valid::Bool(false) => errors.push(error(
                        "parameter-type-invalid",
                        ValidationLevel::Error,
                        format!(
                            "Parameter '{key}' of '{}' must be type of '{}'",
                            scalar_to_js_string(&name_value(f)?).map_err(throw)?,
                            type_to_string(t)
                        ),
                    )),
                    Valid::Errors(e) => errors.extend(e),
                    Valid::Bool(true) => {}
                }
            }
        }
    }

    for (key, param) in schema.parameters.iter().flatten() {
        // `for (const [key, { required }] of Object.entries(schema.parameters ?? {}))`: a
        // `null` schema throws.
        param.kind.read("required").map_err(throw)?;
        // `fn.parameters[key] === undefined`, through the parameters' prototype.
        let undefined = match (f, &parameters) {
            (Scalar::Function(plain), _) => {
                is_undefined_in(&plain.parameters, key)
                    || matches!(plain.parameters.get(key), Some(Scalar::Undefined))
            }
            (_, Some(p)) => value::is_undefined_at(p, key).map_err(throw)?,
            _ => unreachable!("a call"),
        };
        if param.required && undefined {
            errors.push(error(
                "parameter-missing-required",
                ValidationLevel::Error,
                format!("Missing required parameter: '{key}'"),
            ));
        }
    }

    Ok(errors)
}

/// A call's `name` as it is (for the messages that print it: `${fn.name}`).
fn name_value(f: &Scalar) -> Result<Scalar> {
    Ok(match f {
        Scalar::Function(plain) => Scalar::String(plain.name.clone()),
        Scalar::Js(js) => scalar_of(&value::get(js.value(), "name").map_err(throw)?),
        _ => unreachable!("a call"),
    })
}

/// A call as a schema's `validate(fn, config)` takes it (a Rust caller's): itself when it is
/// a [`Scalar::Function`]; else its name and its own parameters as JSON keeps them.
fn as_function(f: &Scalar, name: &str) -> Function {
    match f {
        Scalar::Function(plain) => plain.clone(),
        Scalar::Js(js) => {
            let parameters: IndexMap<String, Scalar> = value::get(js.value(), "parameters")
                .ok()
                .map(|p| {
                    value::own_entries(&p)
                        .into_iter()
                        .map(|(k, v)| (k, v.to_scalar().unwrap_or(Scalar::Null)))
                        .collect()
                })
                .unwrap_or_default();
            Function::new(name.to_string(), parameters)
        }
        _ => unreachable!("a call"),
    }
}

/// `displayMatches(matches, n)` (`Err` where `JSON.stringify` throws: a `RangeError`).
fn display_matches(matches: &[Value], n: usize) -> Result<String> {
    if matches.len() <= n {
        return Ok(value::json_stringify(&Value::array(matches.to_vec()), "")
            .map_err(throw)?
            .unwrap_or_default());
    }
    let items = matches[..n]
        .iter()
        .map(|item| {
            Ok(value::json_stringify(item, "")
                .map_err(throw)?
                .unwrap_or_default())
        })
        .collect::<Result<Vec<String>>>()?;
    Ok(format!(
        "[{}, ... {} more]",
        items.join(","),
        matches.len() - n
    ))
}

/// `matches.includes(value)` (SameValueZero): only a primitive can be in the list; an array,
/// an object or an AST value is a new object, in no list.
fn includes(matches: &[Value], value: &Scalar) -> bool {
    matches.iter().any(|m| match (m, value) {
        (Value::Undefined, Scalar::Undefined) | (Value::Null, Scalar::Null) => true,
        (Value::Boolean(a), Scalar::Boolean(b)) => a == b,
        (Value::Number(a), Scalar::Number(b)) => a == b || (a.is_nan() && b.is_nan()),
        (Value::String(a), Scalar::String(b)) => a == b,
        _ => false,
    })
}

/// `matches = matches(config)` for a built-in function: called with `undefined` as `this` and
/// the config (a plain object) as its argument. What it returns is matched as any `matches`
/// is: an array (of values), a RegExp (none of these functions makes one), else nothing.
fn call_matches(f: crate::next::realm::Builtin) -> Result<Matches> {
    use crate::next::realm::Called;
    let config = Value::empty_object();
    match crate::next::realm::call(f, &Value::Undefined, &[config]).map_err(throw)? {
        // `matches.includes(value)`: a hole is `undefined`.
        Called::Value(Value::Array(items)) => Ok(Matches::Array(
            (0..items.len())
                .map(|i| items.get(i).cloned().unwrap_or(Value::Undefined))
                .collect(),
        )),
        _ => Ok(Matches::Other),
    }
}

/// The attribute schema an inherited name finds: a function with none of the fields.
static INHERITED_ATTRIBUTE: std::sync::LazyLock<SchemaAttribute> =
    std::sync::LazyLock::new(SchemaAttribute::default);

/// Is `config.variables` truthy?
fn variables_truthy(config: &Config) -> bool {
    match &config.variables {
        Some(Variables::Value(v)) => v.to_boolean(),
        Some(Variables::Fn(_)) => true,
        None => false,
    }
}

/// `for (const key of value.path)`: the steps of a variable's path as JavaScript iterates
/// them: an array's elements from 0 to its length (a hole is `undefined`), a string's code
/// points, an object that inherits from an array as that array would be; anything else is
/// not iterable (`TypeError: value.path is not iterable`).
fn path_steps(variable: &Value) -> Result<(Value, Vec<Value>)> {
    let path = value::get(variable, "path").map_err(throw)?;
    let not_iterable = || throw("TypeError: value.path is not iterable");
    let steps = match &path {
        Value::String(s) => char::decode_utf16(s.code_units())
            .map(|c| match c {
                Ok(c) => Value::string(c.to_string()),
                Err(lone) => {
                    Value::String(value::JsString::from_code_unit(lone.unpaired_surrogate()))
                }
            })
            .collect(),
        Value::Array(_) | Value::Object(_) if inherits_array(&path) => {
            let (length, present) = value::array_like_elements(&path).map_err(throw)?;
            let mut steps = Vec::with_capacity(present.len());
            let mut present = present.into_iter().peekable();
            for i in 0..length {
                match present.peek() {
                    Some((j, _)) if *j == i => steps.push(present.next().expect("peeked").1),
                    _ => steps.push(Value::Undefined),
                }
            }
            steps
        }
        _ => return Err(not_iterable()),
    };
    Ok((path, steps))
}

/// Is `Array.prototype` on the value's chain (so that its `Symbol.iterator` is the array
/// iterator)?
fn inherits_array(value: &Value) -> bool {
    let mut at = value::prototype_of(value);
    loop {
        match at {
            Some(value::Proto::Builtin(p)) => return p == BuiltinProto::ARRAY_PROTOTYPE,
            Some(value::Proto::Value(v)) => at = value::prototype_of(&v),
            _ => return false,
        }
    }
}

/// The variable check for a variable the plain variants do not hold (its path read through
/// its chain): whether a step names no own property, and the path's text for the message
/// (`value.path.join('.')`, V8's `TypeError` when its `join` is not `Array.prototype.join`).
fn js_variable_missing(variable: &Value, config: &Config) -> Result<Option<String>> {
    let (path, steps) = path_steps(variable)?;
    let mut variables = match &config.variables {
        Some(Variables::Value(v)) => v.clone(),
        Some(Variables::Fn(_)) => {
            // A function's own properties are its `length`, `name` and `prototype`.
            match steps.first() {
                Some(step) => {
                    let key = value::to_property_key(step).map_err(throw)?;
                    if matches!(key.as_str(), "length" | "name" | "prototype") {
                        return Err(throw(
                            "not modeled: a variable path into a variables function's own properties",
                        ));
                    }
                    return missing_text(&path).map(Some);
                }
                None => return Ok(None),
            }
        }
        None => Value::Undefined,
    };
    for step in &steps {
        let key = value::to_property_key(step).map_err(throw)?;
        if !value::has_own_property(&variables, &key).map_err(throw)? {
            return missing_text(&path).map(Some);
        }
        variables = value::get(&variables, &key).map_err(throw)?;
    }
    Ok(None)
}

/// `value.path.join('.')`.
fn missing_text(path: &Value) -> Result<String> {
    value::array_method(path, "join", value::Method::ArrayJoin, "value.path").map_err(throw)?;
    Ok(value::array_join(path, &Value::string("."))
        .map_err(throw)?
        .into_string_lossy())
}

/// The variable check: does every step of the path name an own property?
fn variable_missing(path: &[crate::ast::PathSegment], config: &Config) -> Result<bool> {
    let mut variables = match &config.variables {
        Some(Variables::Value(v)) => v.clone(),
        Some(Variables::Fn(_)) => {
            // A function's own properties are its `length`, `name` and `prototype`.
            return match path.first().map(value::property_key).as_deref() {
                Some("length" | "name" | "prototype") => Err(throw(
                    "not modeled: a variable path into a variables function's own properties",
                )),
                Some(_) => Ok(true),
                None => Ok(false),
            };
        }
        None => Value::Undefined,
    };
    for step in path {
        let key = value::property_key(step);
        if !value::has_own_property(&variables, &key).map_err(throw)? {
            return Ok(true);
        }
        variables = value::get_property(&variables, &key).map_err(throw)?;
    }
    Ok(false)
}

/// `validator(node, config)`: the node's own errors, then every check its schema asks for.
pub fn validator(node: &Node, cx: &ValidationContext<'_>) -> Result<Vec<ValidationError>> {
    let config = cx.config;
    let schema = find_schema(node, config).get();
    let mut errors: Vec<ValidationError> = node.errors.clone();
    let tag = node.tag.as_deref();
    let has_tag = tag.is_some_and(|t| !t.is_empty());

    let Some(schema) = schema else {
        errors.push(error(
            if has_tag {
                "tag-undefined"
            } else {
                "node-undefined"
            },
            ValidationLevel::Critical,
            if has_tag {
                format!("Undefined tag: '{}'", tag.unwrap_or_default())
            } else {
                format!("Undefined node: '{}'", node.node_type)
            },
        ));
        return Ok(errors);
    };

    // `${node.tag}`: `undefined` when the node has no tag.
    let tag_text = tag.unwrap_or("undefined");

    // `schema.inline != undefined && node.inline !== schema.inline`.
    if let Some(inline) = &schema.inline
        && !matches!(inline, Value::Boolean(b) if *b == node.inline)
    {
        errors.push(error(
            "tag-placement-invalid",
            ValidationLevel::Critical,
            format!(
                "'{tag_text}' tag should be {}",
                if inline.to_boolean() {
                    "inline"
                } else {
                    "block"
                }
            ),
        ));
    }

    if schema.self_closing && !node.children.is_empty() {
        errors.push(error(
            "tag-selfclosing-has-children",
            ValidationLevel::Critical,
            format!("'{tag_text}' tag should be self-closing"),
        ));
    }

    let attributes = schema::merged_attributes(Some(schema));

    for key in node.slots.keys() {
        // `const slot = schema.slots?.[key]; if (!slot) ...`.
        let defined = match &schema.slots {
            Some(slots) => match lookup(slots, key) {
                Found::Own(slot) => !slot.kind.is_falsy(),
                Found::Inherited => true,
                Found::Missing => false,
            },
            None => false,
        };
        if !defined {
            errors.push(error(
                "slot-undefined",
                ValidationLevel::Error,
                format!("Invalid slot: '{key}'"),
            ));
        }
    }

    for (key, value) in &node.attributes {
        // `const attrib = attributes[key]; if (!attrib) ...`.
        let attrib = match lookup(&attributes, key) {
            Found::Own(a) if !a.kind.is_falsy() => a,
            Found::Inherited => &*INHERITED_ATTRIBUTE,
            Found::Own(_) | Found::Missing => {
                errors.push(error(
                    "attribute-undefined",
                    ValidationLevel::Error,
                    format!("Invalid attribute: '{key}'"),
                ));
                continue;
            }
        };

        let level = attrib.error_level.clone().unwrap_or(ValidationLevel::Error);

        if is_ast(value) {
            if is_function(value) && config.validation.validate_functions {
                errors.extend(validate_function(value, cx)?);
            } else if is_variable(value) && variables_truthy(config) {
                let missing = match value {
                    Scalar::Variable(v) => variable_missing(&v.path, config)?.then(|| {
                        let path: Vec<String> = v.path.iter().map(value::property_key).collect();
                        path.join(".")
                    }),
                    Scalar::Js(js) => js_variable_missing(js.value(), config)?,
                    _ => unreachable!("a variable"),
                };
                if let Some(path) = missing {
                    errors.push(error(
                        "variable-undefined",
                        ValidationLevel::Error,
                        format!("Undefined variable: '{path}'"),
                    ));
                }
            } else {
                continue;
            }
        }

        if let Some(t) = &attrib.attr_type {
            match validate_type(t, value, cx, key)? {
                Valid::Bool(false) => errors.push(error(
                    "attribute-type-invalid",
                    level.clone(),
                    format!("Attribute '{key}' must be type of '{}'", type_to_string(t)),
                )),
                Valid::Errors(e) => errors.extend(e),
                Valid::Bool(true) => {}
            }
        }

        let matches = match &attrib.matches {
            Some(Matches::Fn(f)) => Some(f(cx)),
            Some(Matches::Call(f)) => Some(call_matches(*f)?),
            other => other.clone(),
        };

        if let Some(Matches::Array(list)) = &matches
            && !includes(list, value)
        {
            errors.push(error(
                "attribute-value-invalid",
                level.clone(),
                format!(
                    "Attribute '{key}' must match one of {}. Got '{}' instead.",
                    display_matches(list, 8)?,
                    scalar_to_js_string(value).map_err(throw)?
                ),
            ));
        }

        if let Some(Matches::RegExp(regexp)) = &matches
            && !regexp
                .test(&scalar_to_js_string(value).map_err(throw)?)
                .map_err(throw)?
        {
            errors.push(error(
                "attribute-value-invalid",
                level.clone(),
                format!(
                    "Attribute '{key}' must match {}. Got '{}' instead.",
                    regexp.to_js_string(),
                    scalar_to_js_string(value).map_err(throw)?
                ),
            ));
        }

        if let Some(validate) = &attrib.validate {
            errors.extend(validate(value, cx, key)?);
        }
    }

    // `for (const [key, { required }] of Object.entries(attributes))`: a `null` schema throws.
    for (key, attr) in &attributes {
        attr.kind.read("required").map_err(throw)?;
        if attr.required && attribute_undefined(node, key)? {
            errors.push(error(
                "attribute-missing-required",
                ValidationLevel::Error,
                format!("Missing required attribute: '{key}'"),
            ));
        }
    }

    if let Some(slots) = &schema.slots {
        for (key, slot) in slots {
            slot.kind.read("required").map_err(throw)?;
            if slot.required && slot_undefined(node, key) {
                errors.push(error(
                    "slot-missing-required",
                    ValidationLevel::Error,
                    format!("Missing required slot: '{key}'"),
                ));
            }
        }
    }

    for child in &node.children {
        let child_type = child.node_type.as_str();
        // `schema.children && type !== 'error' && !schema.children.includes(type)`.
        if child_type != "error"
            && let Some(allowed) = schema.allows_child(child_type)
            && !allowed.map_err(throw)?
        {
            let parent = match tag {
                Some(t) if !t.is_empty() => t.to_string(),
                _ => node.node_type.to_string(),
            };
            errors.push(error(
                "child-invalid",
                ValidationLevel::Warning,
                format!("Can't nest '{child_type}' in '{parent}'"),
            ));
        }
    }

    if let Some(validate) = &schema.validate {
        errors.extend(validate(node, cx)?);
    }

    Ok(errors)
}

/// `node.attributes[key] === undefined`: an own attribute that is `undefined`, or none, and
/// then the attributes' prototype's (`Object.prototype` unless a `__proto__` attribute set
/// another) says.
fn attribute_undefined(node: &Node, key: &str) -> Result<bool> {
    if let Some(v) = node.attributes.get(key) {
        return Ok(matches!(v, Scalar::Undefined));
    }
    Ok(match node.attributes_prototype.as_deref() {
        None => !value::is_object_prototype_name(key),
        Some(Scalar::Null) => true,
        Some(proto) => value::is_undefined_at(&Value::from_scalar(proto), key).map_err(throw)?,
    })
}

/// `node.slots[key] === undefined`: an own slot, or none, and then the slots' prototype's
/// (`Object.prototype`, or the node a slot named `__proto__` made it) says.
fn slot_undefined(node: &Node, key: &str) -> bool {
    if node.slots.contains_key(key) {
        return false;
    }
    match node.slots_prototype.as_deref() {
        None => is_undefined_in(&node.slots, key),
        Some(proto) => !node_property_defined(proto, key),
    }
}

/// Is `node[key]` not `undefined` for upstream's `Node` object the node is: its own
/// properties (`$$mdtype`, `errors`, `lines`, `inline`, `attributes`, `children`, `type`,
/// `tag`, `annotations`, `slots`, and `location` once the parser gave it one), then
/// `Node.prototype`'s and `Object.prototype`'s.
fn node_property_defined(node: &Node, key: &str) -> bool {
    match key {
        "$$mdtype" | "errors" | "lines" | "inline" | "attributes" | "children" | "type"
        | "annotations" | "slots" => true,
        "tag" => node.tag.is_some(),
        "location" => node.location.is_some(),
        _ => realm::has_inherited(BuiltinProto::NODE_PROTOTYPE, key),
    }
}

/// The deepest node `validate` reaches from the top of a thread: a node with more ancestors
/// than `MAX_VALIDATE_DEPTH - 1` makes it throw upstream's `RangeError` instead.
///
/// Upstream walks the tree with a recursive generator (`walkWithParents`), which overflows
/// V8's stack below a fixed depth: node 22 validates a document whose deepest node is the
/// 4,046th on its path from the root (the document) and throws from the 4,047th, called from
/// the top of a script, in a fresh process and after warming up alike, whatever the nodes are
/// (tags, blockquotes, lists, slots, siblings at every level); called from 50 frames deeper it
/// throws from the 4,024th. This port walks with a stack of its own and charges each node on
/// the path `VALIDATE_COST` of what is left of the depth budget ([`crate::depth`]).
pub const MAX_VALIDATE_DEPTH: usize = 4046;

/// What a node on the path of `validate`'s walk costs on the depth budget: a generator frame
/// of `walkWithParents` (see [`MAX_VALIDATE_DEPTH`]).
pub(crate) const VALIDATE_COST: u64 = crate::depth::per_level(MAX_VALIDATE_DEPTH as u64);

/// The error upstream throws past a recursion limit.
pub(crate) const STACK_OVERFLOW: &str = "RangeError: Maximum call stack size exceeded";

/// `walkWithParents(node)`: each node with its ancestors (the root first), depth first, a
/// node's slots before its children; `visit` stops the walk with an error. Upstream's
/// generator yields a fresh copy of the ancestors for every node (memory quadratic in the
/// depth); here `visit` borrows the one path the walk keeps, and the walk keeps its own stack
/// instead of recursing.
pub fn walk_with_parents<'a, E>(
    node: &'a Node,
    mut visit: impl FnMut(&'a Node, &[&'a Node]) -> std::result::Result<(), E>,
) -> std::result::Result<(), E> {
    // The path from the root to the node being walked, and for each node on it the index of
    // its next slot or child (slots first).
    let mut path: Vec<&'a Node> = Vec::new();
    let mut next: Vec<usize> = Vec::new();
    visit(node, &path)?;
    path.push(node);
    next.push(0);
    while let (Some(&current), Some(k)) = (path.last(), next.last_mut()) {
        let child = match current.slots.get_index(*k) {
            Some((_, slot)) => Some(slot),
            None => current.children.get(*k - current.slots.len()),
        };
        match child {
            Some(child) => {
                *k += 1;
                visit(child, &path)?;
                path.push(child);
                next.push(0);
            }
            None => {
                path.pop();
                next.pop();
            }
        }
    }
    Ok(())
}

/// Is some node of the tree the `limit + 1`th on its path from the root?
pub(crate) fn deeper_than(node: &Node, limit: usize) -> bool {
    struct Deep;
    walk_with_parents(node, |_, parents| {
        if parents.len() >= limit {
            Err(Deep)
        } else {
            Ok(())
        }
    })
    .is_err()
}

/// `hasValidLocation(error)`: the lines of an error's `location` whose edges both have a
/// numeric `line` and whose `file` is a string or undefined.
fn valid_location_lines(location: Option<&Json>) -> Option<[f64; 2]> {
    let location = location?;
    // `loc.start?.line`: the property of an object (a primitive's, an array's is undefined).
    let line = |edge: &str| {
        location
            .get(edge)?
            .as_object()?
            .get("line")
            .and_then(Json::as_f64)
    };
    let lines = [line("start")?, line("end")?];
    match location.get("file") {
        None | Some(Json::String(_)) => Some(lines),
        Some(_) => None,
    }
}

/// `toValidateError(node, error)`: an error with a valid location (see
/// [`valid_location_lines`]) is placed by it, `{type, lines: [start.line, end.line],
/// location: {file: node.location?.file, ...error.location}, error}`; any other by its
/// node, `{type, lines: node.lines, location: node.location, error}`.
///
/// `Err` where upstream's `error.location` throws: an error that is `null` (a hand-made
/// token's `errors` entry, [`ValidationError::raw`]).
fn to_validate_error(node: &Node, error: ValidationError) -> Result<ValidateError> {
    if error.raw.as_deref() == Some(&Json::Null) {
        return Err(throw(
            "TypeError: Cannot read properties of null (reading 'location')",
        ));
    }
    let span = error.span.clone().or_else(|| node.span.clone());
    if let Some([start, end]) = valid_location_lines(error.location.as_ref()) {
        // The object literal's `file` comes first; the spread overwrites its value when the
        // error's location has its own (a key with no value does not exist in JSON).
        let mut file = node
            .location
            .as_ref()
            .and_then(|l| l.file.as_deref())
            .map(|f| Json::String(f.to_string()));
        let mut rest = Vec::new();
        for (key, value) in error.location.iter().flat_map(|l| l.as_object()).flatten() {
            if key == "file" {
                file = Some(value.clone());
            } else {
                rest.push((key.clone(), value.clone()));
            }
        }
        let location = js_object(
            file.map(|f| ("file".to_string(), f))
                .into_iter()
                .chain(rest),
        );
        return Ok(ValidateError {
            node_type: node.node_type.clone(),
            lines: vec![start, end],
            location: Some(ValidateLocation::Error(location)),
            error,
            span,
        });
    }
    Ok(ValidateError {
        node_type: node.node_type.clone(),
        lines: node.lines.iter().map(|&l| l as f64).collect(),
        location: node.location.clone().map(ValidateLocation::Node),
        error,
        span,
    })
}

/// A JS object of `entries` (in order, but array-index keys first, ascending) as JSON.
fn js_object(entries: impl Iterator<Item = (String, Json)>) -> Json {
    let mut entries: Vec<(String, Json)> = entries.collect();
    let rank = |key: &str| crate::types::array_index(key).map_or((1, 0), |n| (0, n));
    entries.sort_by_key(|(key, _)| rank(key));
    Json::Object(entries.into_iter().collect())
}

/// `validateTree(content, config)`: every node's errors, in walk order. `config` is the merged
/// config (see [`crate::next::index::validate`]).
///
/// Upstream collects the whole walk before it validates a node, so a tree too deep for the
/// walk ([`MAX_VALIDATE_DEPTH`] from the top of a thread; less where the calls below charge
/// the depth budget) throws before any node is validated (and before any `matches` RegExp
/// moves its `lastIndex`).
pub fn validate_tree(content: &Node, config: &Config) -> Result<Vec<ValidateError>> {
    let limit = usize::try_from(crate::depth::left() / VALIDATE_COST).unwrap_or(usize::MAX);
    if deeper_than(content, limit) {
        return Err(throw(STACK_OVERFLOW));
    }
    let mut output = Vec::new();
    walk_with_parents(content, |node, parents| {
        let cx = ValidationContext { config, parents };
        for error in validator(node, &cx)? {
            output.push(to_validate_error(node, error)?);
        }
        Ok(())
    })?;
    Ok(output)
}

#[cfg(test)]
mod tests;
