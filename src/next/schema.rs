//! Port of upstream Markdoc v0.5.11's `src/schema.ts` and `src/schema-types/*`.
//!
//! Owner: the semantics agent.
//!
//! The default node schemas ([`default_nodes`]), the `Class`, `Id` and
//! `ConditionalAttributeType` types, and the new layer's `Config`/`Schema`/`SchemaAttribute`
//! types (upstream's `types.ts`).
//!
//! The types hold what upstream's JavaScript objects can hold, with JavaScript's semantics
//! where upstream's code relies on them:
//!
//! - An object upstream keeps a map in (`config.tags`, a schema's `attributes`, ...) is an
//!   [`IndexMap`] in a JS object's key order, and a lookup in one also finds what every JS
//!   object inherits from `Object.prototype` ([`lookup`]): upstream finds a schema for a tag
//!   named `toString`, so this does too.
//! - A value upstream reads only for its truthiness is a `bool` (`required`, `selfClosing`);
//!   one it compares loosely or strictly keeps its value (`render: Option<Value>`, read by
//!   [`renders`] and [`SchemaSlot::renders`]).
//! - `undefined` is `None` (`default`, `variables`, `partials`).
//! - A JS function in a config (a schema's `transform` and `validate`, a function's
//!   `transform`, `matches` as a function, `variables` as a function, a custom attribute type)
//!   is a Rust closure or trait object.
//!
//! [`Config::from_json`] reads a config written as JSON (types as names, `{"$regex", "flags"}`
//! for a RegExp, `{"$type": name}` for a constructor or one of upstream's classes).

pub mod json;
pub mod regexp;
pub mod value;

pub use regexp::{JsMatch, JsRegExp};
pub use value::{TagValue, Value};

use crate::ast::{Function, Node, PathSegment};
use crate::next::transformer::ResolvedNode;
use crate::types::{MarkdocError, Result, Scalar, ValidationError, ValidationLevel};
use indexmap::IndexMap;
use std::borrow::Cow;
use std::fmt;
use std::sync::{Arc, LazyLock};

/// A schema's `transform(node, config)`.
pub type TransformFn = Arc<dyn Fn(&ResolvedNode<'_>, &Config) -> Result<Value> + Send + Sync>;
/// A schema's `validate(node, config)`; `config.validation.parents` is
/// [`ValidationContext::parents`].
pub type ValidateFn =
    Arc<dyn Fn(&Node, &ValidationContext<'_>) -> Result<Vec<ValidationError>> + Send + Sync>;
/// An attribute's `validate(value, config, name)`.
pub type AttributeValidateFn = Arc<
    dyn Fn(&Scalar, &ValidationContext<'_>, &str) -> Result<Vec<ValidationError>> + Send + Sync,
>;
/// A configured function's `transform(parameters, config)`: the parameters as upstream passes
/// them, `resolve(fn.parameters, config)`, which is normally a plain object of the call's
/// arguments (keyed by name or position) but any value a document can make (an object with a
/// prototype, or `true` for `class=f() .parameters`): read it as JavaScript does
/// ([`value::get`] for `parameters[0]`, [`value::own_values_of`] for
/// `Object.values(parameters)`).
pub type FunctionTransformFn = Arc<dyn Fn(&Value, &Config) -> Result<Value> + Send + Sync>;
/// A configured function's `validate(fn, config)`.
pub type FunctionValidateFn =
    Arc<dyn Fn(&Function, &ValidationContext<'_>) -> Result<Vec<ValidationError>> + Send + Sync>;
/// `matches` given as a function of the config.
pub type MatchesFn = Arc<dyn Fn(&ValidationContext<'_>) -> Matches + Send + Sync>;
/// `variables` given as a function of a variable's path.
pub type VariablesFn = Arc<dyn Fn(&[PathSegment]) -> Value + Send + Sync>;

/// The config a validate callback sees: upstream passes `{...config, validation:
/// {...config.validation, parents}}`, the node's ancestors from the root down.
#[derive(Clone, Copy)]
pub struct ValidationContext<'a> {
    pub config: &'a Config,
    pub parents: &'a [&'a Node],
}

/// Upstream's `Config` (`types.ts`).
#[derive(Clone, Default)]
pub struct Config {
    /// Node schemas by node type. `None` is an entry whose value is falsy (`null`), which
    /// hides a default schema of that name.
    pub nodes: IndexMap<String, Option<Schema>>,
    /// Tag schemas by tag name; `None` as for `nodes`.
    pub tags: IndexMap<String, Option<Schema>>,
    /// `None` when the config has no `variables` (`undefined`).
    pub variables: Option<Variables>,
    /// Functions by name; `None` is an entry whose value is falsy.
    pub functions: IndexMap<String, Option<ConfigFunction>>,
    /// `None` when the config has no `partials`.
    pub partials: Option<IndexMap<String, Partial>>,
    /// `partials` is `null` (a config read from JSON can give it): reading a partial from it
    /// throws (`const { partials = {} } = config` keeps a `null`).
    pub partials_null: bool,
    pub validation: Validation,
}

/// `config.variables`: a value (normally an object), or a function of the path.
#[derive(Clone)]
pub enum Variables {
    Value(Value),
    Fn(VariablesFn),
}

/// `config.validation` (its `parents` are [`ValidationContext::parents`]).
#[derive(Debug, Clone, Default)]
pub struct Validation {
    pub validate_functions: bool,
    pub environment: Option<String>,
}

/// A `config.partials` entry: a parsed document, a list of them, or any other value (what a
/// config read from JSON holds: the partial's source, which `validate` only tests for
/// truthiness and `transform` cannot use).
#[derive(Debug, Clone)]
pub enum Partial {
    Node(Arc<Node>),
    Nodes(Arc<Vec<Node>>),
    Value(Value),
}

/// Upstream's `Schema`.
#[derive(Clone, Default)]
pub struct Schema {
    /// `render`: the name of the tag the node renders as (`new Tag(schema.render, ...)`,
    /// whatever value it is: a config read from JSON can give a number or an object); `None`
    /// for none or a falsy one (the node renders its children).
    pub render: Option<Value>,
    /// `children`: what `schema.children.includes(type)` searches (an array's items, a
    /// string's text: see [`Schema::allows_child`]); `None` for none or a falsy one.
    pub children: Option<Value>,
    /// `{...globalAttributes, ...schema.attributes}`'s part: the attributes' schemas by name.
    pub attributes: Option<IndexMap<String, SchemaAttribute>>,
    pub slots: Option<IndexMap<String, SchemaSlot>>,
    pub self_closing: bool,
    /// `inline`: `None` is `undefined` or `null` (either placement is fine); any other value
    /// is compared with the node's by `!==` (so only `true` or `false` can ever match).
    pub inline: Option<Value>,
    pub transform: Option<TransformFn>,
    pub validate: Option<ValidateFn>,
    pub description: Option<String>,
}

/// What a schema value that upstream reads fields of is, when a config read from JSON gives
/// something other than an object there (an attribute's, a slot's or a function parameter's
/// schema): upstream reads its fields all the same.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SchemaKind {
    /// An object: its fields are the schema's.
    #[default]
    Object,
    /// A truthy value that has none of the fields (`true`, `1`, a string, an array): each
    /// reads as `undefined`.
    Truthy,
    /// A falsy value other than `null` (`false`, `0`, `""`): each field reads as `undefined`,
    /// and `!schema` is true (validate() calls the name undefined).
    Falsy,
    /// `null`: reading a field throws (`TypeError: Cannot read properties of null (reading
    /// 'render')`).
    Null,
}

impl SchemaKind {
    /// `!schema`.
    pub fn is_falsy(self) -> bool {
        matches!(self, SchemaKind::Falsy | SchemaKind::Null)
    }

    /// `schema.field` (and `const { field } = schema`): `Err` (V8's `TypeError`) for `null`.
    pub fn read(self, field: &str) -> std::result::Result<(), String> {
        match self {
            SchemaKind::Null => Err(format!(
                "TypeError: Cannot read properties of null (reading '{field}')"
            )),
            _ => Ok(()),
        }
    }
}

/// Upstream's `SchemaAttribute` (also a function parameter's schema).
#[derive(Clone, Default)]
pub struct SchemaAttribute {
    /// What the schema is when it is not an object (its fields are then all `None`).
    pub kind: SchemaKind,
    /// `None` for no type or a falsy one.
    pub attr_type: Option<TypeSpec>,
    /// `undefined` (`None`) renders under the attribute's own name, as does any value but a
    /// string (the name to render as) or one that is loosely `== false` (not rendered).
    pub render: Option<Value>,
    /// `None` is `undefined`: no default.
    pub default: Option<Value>,
    pub required: bool,
    pub matches: Option<Matches>,
    pub validate: Option<AttributeValidateFn>,
    /// `None` for none or a falsy one (`'error'`).
    pub error_level: Option<ValidationLevel>,
    pub description: Option<String>,
}

/// Upstream's `SchemaSlot`.
#[derive(Debug, Clone, Default)]
pub struct SchemaSlot {
    /// What the schema is when it is not an object (its fields are then all `None`).
    pub kind: SchemaKind,
    /// Only `false` hides the slot; a string renders it under that name.
    pub render: Option<Value>,
    pub required: bool,
}

/// Upstream's `ConfigFunction`.
#[derive(Clone, Default)]
pub struct ConfigFunction {
    /// `returns` as a list (a single type is a list of one); `None` when it is falsy.
    pub returns: Option<Vec<TypeSpec>>,
    pub parameters: Option<IndexMap<String, SchemaAttribute>>,
    pub transform: Option<FunctionTransformFn>,
    pub validate: Option<FunctionValidateFn>,
    pub description: Option<String>,
}

/// A `type`: upstream's `ValidationType | ValidationType[]`.
#[derive(Clone)]
pub enum TypeSpec {
    /// A falsy entry in a list of types (`null`, `false`, `0`, `''`): it accepts any value.
    Falsy,
    /// A type by name (`'String'`), looked up in upstream's `TypeMappings`.
    Name(String),
    /// A JavaScript constructor (`String`, `Number`, ...).
    Builtin(BuiltinType),
    /// One of upstream's own classes.
    Markdoc(MarkdocType),
    /// A custom attribute type (a class with `transform` and/or `validate`).
    Custom(Arc<dyn CustomType>),
    /// A list of types (a JS array: a new object, `===` to nothing else).
    Union(Vec<TypeSpec>),
    /// Any other truthy value a config read from JSON gives as a type (a number, an object
    /// that is not a type): no value's `constructor` is it, and `typeToString` prints its
    /// `name` (`undefined` for most).
    Value(Value),
    /// Any other built-in object of the realm (a config written as JSON names one with
    /// `{"$type": name}` for a name every object inherits): a function that is no
    /// constructor (`new` of it throws), or `Object.prototype`, no function at all (no value's
    /// `constructor` is it).
    Realm(crate::next::realm::Builtin),
}

/// The constructors upstream's `TypeMappings` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinType {
    String,
    Number,
    Boolean,
    Object,
    Array,
}

/// Upstream's own attribute type classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkdocType {
    /// `schema-types/class.ts`.
    Class,
    /// `schema-types/id.ts`.
    Id,
    /// `schema-types/conditional.ts`.
    ConditionalAttributeType,
    /// `PartialFile` in `tags/partial.ts`.
    PartialFile,
}

/// A custom attribute type (upstream's `CustomAttributeType`): a class whose instance may
/// have `transform` and `validate`. Return `None` for a method the class does not have.
pub trait CustomType: Send + Sync {
    /// The class's name (`type.name`), shown in "must be type of" messages.
    fn name(&self) -> &str;
    /// `instance.transform(value, config)`.
    fn transform(&self, value: Value, config: &Config) -> Option<Result<Value>> {
        let _ = (value, config);
        None
    }
    /// `instance.validate(value, config, key)`.
    fn validate(
        &self,
        value: &Scalar,
        cx: &ValidationContext<'_>,
        key: &str,
    ) -> Option<Result<Vec<ValidationError>>> {
        let _ = (value, cx, key);
        None
    }
}

/// An attribute's `matches`.
#[derive(Clone)]
pub enum Matches {
    /// The values the attribute may hold (compared as `Array.prototype.includes` compares).
    Array(Vec<Value>),
    RegExp(Arc<JsRegExp>),
    /// A function of the config, returning one of the others.
    Fn(MatchesFn),
    /// A built-in function (a config written as JSON names one with `{"$type": name}`):
    /// upstream calls it, `matches = matches(config)`, with `undefined` as `this`.
    Call(crate::next::realm::Builtin),
    /// Any other value (`null`, a string, an object): nothing to match.
    Other,
}

impl BuiltinType {
    /// The constructor's `name`.
    pub fn name(self) -> &'static str {
        match self {
            BuiltinType::String => "String",
            BuiltinType::Number => "Number",
            BuiltinType::Boolean => "Boolean",
            BuiltinType::Object => "Object",
            BuiltinType::Array => "Array",
        }
    }

    /// `TypeMappings[name]`, for the names it holds itself.
    pub fn from_name(name: &str) -> Option<BuiltinType> {
        Some(match name {
            "String" => BuiltinType::String,
            "Number" => BuiltinType::Number,
            "Boolean" => BuiltinType::Boolean,
            "Object" => BuiltinType::Object,
            "Array" => BuiltinType::Array,
            _ => return None,
        })
    }
}

impl MarkdocType {
    /// The class's `name`.
    pub fn name(self) -> &'static str {
        match self {
            MarkdocType::Class => "Class",
            MarkdocType::Id => "Id",
            MarkdocType::ConditionalAttributeType => "ConditionalAttributeType",
            MarkdocType::PartialFile => "PartialFile",
        }
    }

    /// `instance.transform(value, config)`, for the classes that have one (`Class`).
    pub fn transform(self, value: Value) -> Option<Value> {
        match self {
            MarkdocType::Class => Some(class_transform(value)),
            _ => None,
        }
    }

    /// `instance.validate(value, config, key)`: every one of upstream's classes has one.
    /// `Err` where it throws (`PartialFile` reading `partials[file]`).
    pub fn validate(
        self,
        value: &Scalar,
        config: &Config,
        key: &str,
    ) -> std::result::Result<Vec<ValidationError>, String> {
        Ok(match self {
            MarkdocType::Class => class_validate(value, key),
            MarkdocType::Id => id_validate(value),
            MarkdocType::ConditionalAttributeType => conditional_validate(value, key),
            MarkdocType::PartialFile => {
                crate::next::tags::partial::partial_file_validate(value, config)?
            }
        })
    }
}

impl TypeSpec {
    /// `typeof type === 'function'`: a constructor or a class.
    pub fn is_function(&self) -> bool {
        match self {
            TypeSpec::Builtin(_) | TypeSpec::Markdoc(_) | TypeSpec::Custom(_) => true,
            TypeSpec::Realm(b) => b.is_callable(),
            _ => false,
        }
    }

    /// `a === b` for two types: names by value, constructors and classes by identity, a list
    /// never (it is a new array).
    pub fn identical(&self, other: &TypeSpec) -> bool {
        match (self, other) {
            (TypeSpec::Name(a), TypeSpec::Name(b)) => a == b,
            (TypeSpec::Builtin(a), TypeSpec::Builtin(b)) => a == b,
            (TypeSpec::Markdoc(a), TypeSpec::Markdoc(b)) => a == b,
            (TypeSpec::Realm(a), TypeSpec::Realm(b)) => a == b,
            (TypeSpec::Custom(a), TypeSpec::Custom(b)) => {
                std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b))
            }
            (TypeSpec::Value(a), TypeSpec::Value(b)) => value::strict_equals(a, b),
            _ => false,
        }
    }
}

impl fmt::Debug for TypeSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeSpec::Falsy => f.write_str("Falsy"),
            TypeSpec::Name(n) => write!(f, "Name({n:?})"),
            TypeSpec::Builtin(b) => write!(f, "Builtin({})", b.name()),
            TypeSpec::Markdoc(m) => write!(f, "Markdoc({})", m.name()),
            TypeSpec::Custom(c) => write!(f, "Custom({})", c.name()),
            TypeSpec::Union(types) => f.debug_tuple("Union").field(types).finish(),
            TypeSpec::Value(v) => f.debug_tuple("Value").field(v).finish(),
            TypeSpec::Realm(b) => write!(f, "Realm({b:?})"),
        }
    }
}

impl fmt::Debug for Matches {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Matches::Array(values) => f.debug_tuple("Array").field(values).finish(),
            Matches::RegExp(r) => write!(f, "RegExp({})", r.to_js_string()),
            Matches::Fn(_) => f.write_str("Fn(<function>)"),
            Matches::Call(b) => write!(f, "Call({b:?})"),
            Matches::Other => f.write_str("Other"),
        }
    }
}

fn function_marker<T>(f: &Option<T>) -> Option<&'static str> {
    f.as_ref().map(|_| "<function>")
}

impl fmt::Debug for Schema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Schema")
            .field("render", &self.render)
            .field("children", &self.children)
            .field("attributes", &self.attributes)
            .field("slots", &self.slots)
            .field("self_closing", &self.self_closing)
            .field("inline", &self.inline)
            .field("transform", &function_marker(&self.transform))
            .field("validate", &function_marker(&self.validate))
            .finish()
    }
}

impl fmt::Debug for SchemaAttribute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SchemaAttribute")
            .field("attr_type", &self.attr_type)
            .field("render", &self.render)
            .field("default", &self.default)
            .field("required", &self.required)
            .field("matches", &self.matches)
            .field("validate", &function_marker(&self.validate))
            .field("error_level", &self.error_level)
            .finish()
    }
}

impl fmt::Debug for ConfigFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigFunction")
            .field("returns", &self.returns)
            .field("parameters", &self.parameters)
            .field("transform", &function_marker(&self.transform))
            .field("validate", &function_marker(&self.validate))
            .finish()
    }
}

impl fmt::Debug for Variables {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Variables::Value(v) => f.debug_tuple("Value").field(v).finish(),
            Variables::Fn(_) => f.write_str("Fn(<function>)"),
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("nodes", &self.nodes)
            .field("tags", &self.tags)
            .field("variables", &self.variables)
            .field("functions", &self.functions)
            .field("partials", &self.partials)
            .field("validation", &self.validation)
            .finish()
    }
}

/// What a JS property read finds in a map upstream keeps in a plain object.
#[derive(Debug)]
pub enum Found<'a, T> {
    /// An own entry.
    Own(&'a T),
    /// No own entry, but a plain object inherits the name from `Object.prototype` (a
    /// function, or for `__proto__` the prototype itself): truthy, with no property upstream
    /// reads (no `render`, `type`, `transform`, `required`, ...).
    Inherited,
    /// `undefined`.
    Missing,
}

/// What a plain object (with `Object.prototype` as its prototype) inherits under `key`:
/// `Object.prototype`'s property (`toString` is `Object.prototype.toString`, `constructor` is
/// `Object`, `__proto__` the prototype itself), `undefined` for a name it does not have. This
/// is the value [`Found::Inherited`] stands for.
pub fn inherited(key: &str) -> Value {
    crate::next::realm::inherited_by_plain_object(key).unwrap_or(Value::Undefined)
}

/// `map[key]` for a map upstream keeps in a plain object.
pub fn lookup<'a, T>(map: &'a IndexMap<String, T>, key: &str) -> Found<'a, T> {
    match map.get(key) {
        Some(v) => Found::Own(v),
        None if value::is_object_prototype_name(key) => Found::Inherited,
        None => Found::Missing,
    }
}

/// `map[key] === undefined` for a map upstream keeps in a plain object (an inherited name is
/// a function, not `undefined`).
pub fn is_undefined_in<T>(map: &IndexMap<String, T>, key: &str) -> bool {
    matches!(lookup(map, key), Found::Missing)
}

/// `value == false` (loose equality), which is how upstream reads an attribute's `render`:
/// `false`, `0`, `''`, a string that reads as the number 0 (`'0'`, `' '`, `'0x0'`), and an
/// array whose items join to one of those (`[]`, `[0]`) are all `== false`; `null` and
/// `undefined` are not. `Err` where converting the value throws.
pub fn loose_equals_false(value: &Value) -> std::result::Result<bool, String> {
    Ok(match value {
        Value::Undefined | Value::Null => false,
        Value::Boolean(b) => !*b,
        Value::Number(n) => *n == 0.0,
        Value::String(s) => string_to_number(s) == 0.0,
        // `IsLooselyEqual(object, 0)`: the object's primitive (`ToPrimitive` with no hint:
        // `valueOf`, then `toString`) compared again.
        Value::Array(_) | Value::Object(_) | Value::Tag(_) | Value::Builtin(_) => {
            loose_equals_false(&value::to_primitive(value, value::Hint::Default)?)?
        }
    })
}

/// `Number(s)`, over the string's code units.
fn string_to_number(s: &value::JsString) -> f64 {
    crate::markdown_it::js::string_to_number(&s.to_utf16())
}

/// How an attribute renders (transformer.ts): `attr.render == false` hides it; otherwise
/// `typeof attr.render === 'string' ? attr.render : key`. The name is a key, a Rust string:
/// a lone surrogate in it (only a config built in Rust can hold one) is U+FFFD. `Err` where
/// reading `render` throws (an attribute's schema that is `null`).
pub fn renders<'a>(
    attr: &'a SchemaAttribute,
    key: &'a str,
) -> std::result::Result<Option<Cow<'a, str>>, String> {
    attr.kind.read("render")?;
    if let Some(r) = &attr.render
        && loose_equals_false(r)?
    {
        return Ok(None);
    }
    Ok(match &attr.render {
        Some(Value::String(name)) => Some(name.to_string_lossy()),
        _ => Some(Cow::Borrowed(key)),
    })
}

impl SchemaSlot {
    /// `slot.render === false` hides the slot; otherwise `typeof slot.render === 'string' ?
    /// slot.render : key` (a lone surrogate in the name as U+FFFD, as for [`renders`]). `Err`
    /// where reading `render` throws (a slot's schema that is `null`).
    pub fn renders<'a>(
        &'a self,
        key: &'a str,
    ) -> std::result::Result<Option<Cow<'a, str>>, String> {
        self.kind.read("render")?;
        Ok(match &self.render {
            Some(Value::Boolean(false)) => None,
            Some(Value::String(name)) => Some(name.to_string_lossy()),
            _ => Some(Cow::Borrowed(key)),
        })
    }
}

impl Schema {
    /// `schema.children.includes(type)` (with `schema.children` truthy): an array's
    /// `Array.prototype.includes` (SameValueZero), a string's `String.prototype.includes` (the
    /// type a substring of it), V8's `TypeError: schema.children.includes is not a function`
    /// for any other value. `None` when the schema has no `children`.
    pub fn allows_child(&self, child_type: &str) -> Option<std::result::Result<bool, String>> {
        let children = self.children.as_ref()?;
        Some(match children {
            Value::Array(items) => Ok(items
                .present()
                .any(|(_, item)| item.as_str() == Some(child_type))),
            Value::String(s) => {
                let needle: Vec<u16> = child_type.encode_utf16().collect();
                Ok(needle.is_empty() || s.to_utf16().windows(needle.len()).any(|w| w == needle))
            }
            _ => Err("TypeError: schema.children.includes is not a function".into()),
        })
    }
}

// ---------------------------------------------------------------------------------------
// schema-types/class.ts, id.ts, conditional.ts
// ---------------------------------------------------------------------------------------

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

/// `typeof value === 'object'` for an AST value (`null` included, as in JS; `undefined` not).
fn is_js_object(value: &Scalar) -> bool {
    matches!(
        value,
        Scalar::Null
            | Scalar::Array(_)
            | Scalar::Object(_)
            | Scalar::Variable(_)
            | Scalar::Function(_)
            | Scalar::Tag(_)
            | Scalar::Js(_)
    )
}

/// `Class.validate(value, _config, key)`.
pub fn class_validate(value: &Scalar, key: &str) -> Vec<ValidationError> {
    if matches!(value, Scalar::String(_)) || is_js_object(value) {
        return Vec::new();
    }
    vec![error(
        "attribute-type-invalid",
        ValidationLevel::Error,
        format!("Attribute '{key}' must be type 'string | object'"),
    )]
}

/// `Class.transform(value)`: a falsy value or a string as it is, anything else as the keys of
/// its truthy entries joined with spaces.
pub fn class_transform(value: Value) -> Value {
    if !value.to_boolean() || matches!(value, Value::String(_)) {
        return value;
    }
    let classes: Vec<String> = value::own_entries(&value)
        .into_iter()
        .filter(|(_, v)| v.to_boolean())
        .map(|(k, _)| k)
        .collect();
    Value::string(classes.join(" "))
}

/// `Id.validate(value)`: a string starting with an ASCII letter (`/^[a-zA-Z]/`).
pub fn id_validate(value: &Scalar) -> Vec<ValidationError> {
    if let Scalar::String(s) = value
        && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
    {
        return Vec::new();
    }
    vec![error(
        "attribute-value-invalid",
        ValidationLevel::Error,
        "The 'id' attribute must start with a letter".into(),
    )]
}

/// `ConditionalAttributeType.validate(value, _config, key)`: a boolean, `null`, `undefined` or
/// an object.
pub fn conditional_validate(value: &Scalar, key: &str) -> Vec<ValidationError> {
    if matches!(value, Scalar::Boolean(_)) || is_js_object(value) {
        return Vec::new();
    }
    vec![error(
        "attribute-type-invalid",
        ValidationLevel::Error,
        format!(
            "Attribute '{key}' must be type 'boolean | object' (null or undefined are also allowed)"
        ),
    )]
}

// ---------------------------------------------------------------------------------------
// schema.ts: the default node schemas
// ---------------------------------------------------------------------------------------

fn names(list: &[&str]) -> Option<Value> {
    Some(Value::array(
        list.iter().map(|s| Value::string(*s)).collect(),
    ))
}

fn attrs(list: Vec<(&str, SchemaAttribute)>) -> Option<IndexMap<String, SchemaAttribute>> {
    let mut out = IndexMap::new();
    for (k, v) in list {
        crate::types::js_insert(&mut out, k, v);
    }
    Some(out)
}

/// `{type, render?, required?, default?}` with a constructor as the type.
fn attr(t: BuiltinType) -> SchemaAttribute {
    SchemaAttribute {
        attr_type: Some(TypeSpec::Builtin(t)),
        ..Default::default()
    }
}

fn hidden(mut a: SchemaAttribute) -> SchemaAttribute {
    a.render = Some(Value::Boolean(false));
    a
}

fn required(mut a: SchemaAttribute) -> SchemaAttribute {
    a.required = true;
    a
}

fn render_as(mut a: SchemaAttribute, name: &str) -> SchemaAttribute {
    a.render = Some(Value::string(name));
    a
}

fn render(name: &str) -> Option<Value> {
    Some(Value::string(name))
}

fn transform_fn(f: fn(&ResolvedNode<'_>, &Config) -> Result<Value>) -> Option<TransformFn> {
    Some(Arc::new(f))
}

/// `document`.
pub fn document() -> Schema {
    Schema {
        render: render("article"),
        children: names(&[
            "heading",
            "paragraph",
            "image",
            "table",
            "tag",
            "fence",
            "blockquote",
            "comment",
            "list",
            "hr",
        ]),
        attributes: attrs(vec![(
            "frontmatter",
            SchemaAttribute {
                render: Some(Value::Boolean(false)),
                ..Default::default()
            },
        )]),
        ..Default::default()
    }
}

/// `new Tag(name, attributes, children)`. (A function of its own so that the frame of a
/// schema transform, which a deep tree keeps on the stack for each level, stays small.)
#[inline(never)]
fn tag_value(
    name: impl Into<value::JsString>,
    attributes: value::Object,
    children: Vec<Value>,
) -> Value {
    TagValue::new(Value::String(name.into()), attributes, children).into()
}

/// `heading`: `h${level}`.
pub fn heading() -> Schema {
    Schema {
        children: names(&["inline"]),
        attributes: attrs(vec![("level", required(hidden(attr(BuiltinType::Number))))]),
        transform: transform_fn(|node, config| {
            let level = node.attribute("level")?;
            // `h${level}`, code unit by code unit.
            let name = value::JsString::from("h")
                .concat(&level.to_js_string().map_err(MarkdocError::TransformError)?);
            let attributes = node.transform_attributes(config)?;
            let children = node.transform_children(config)?;
            Ok(tag_value(name, attributes, children))
        }),
        ..Default::default()
    }
}

/// `paragraph`.
pub fn paragraph() -> Schema {
    Schema {
        render: render("p"),
        children: names(&["inline"]),
        ..Default::default()
    }
}

/// `image`.
pub fn image() -> Schema {
    Schema {
        render: render("img"),
        attributes: attrs(vec![
            ("src", required(attr(BuiltinType::String))),
            ("alt", attr(BuiltinType::String)),
            ("title", attr(BuiltinType::String)),
        ]),
        ..Default::default()
    }
}

/// `fence`: `pre`, its children or else its `content`.
pub fn fence() -> Schema {
    let mut process = hidden(attr(BuiltinType::Boolean));
    process.default = Some(Value::Boolean(true));
    Schema {
        render: render("pre"),
        attributes: attrs(vec![
            ("content", required(hidden(attr(BuiltinType::String)))),
            (
                "language",
                render_as(attr(BuiltinType::String), "data-language"),
            ),
            ("process", process),
        ]),
        transform: transform_fn(|node, config| {
            // What a fence's transform costs node's stack beyond a heading's.
            let _level = crate::next::transformer::charge(crate::next::transformer::cost::FENCE)?;
            let attributes = node.transform_attributes(config)?;
            let children = if !node.children.is_empty() {
                node.transform_children(config)?
            } else {
                vec![node.attribute("content")?]
            };
            Ok(TagValue::new("pre", attributes, children).into())
        }),
        ..Default::default()
    }
}

/// `blockquote`.
pub fn blockquote() -> Schema {
    Schema {
        render: render("blockquote"),
        children: names(&[
            "heading",
            "paragraph",
            "image",
            "table",
            "tag",
            "fence",
            "blockquote",
            "list",
            "hr",
        ]),
        ..Default::default()
    }
}

/// `item`.
pub fn item() -> Schema {
    Schema {
        render: render("li"),
        children: names(&[
            "inline",
            "heading",
            "paragraph",
            "image",
            "table",
            "tag",
            "fence",
            "blockquote",
            "list",
            "hr",
        ]),
        ..Default::default()
    }
}

/// `list`: `ol` or `ul`.
pub fn list() -> Schema {
    Schema {
        children: names(&["item"]),
        attributes: attrs(vec![
            ("ordered", required(hidden(attr(BuiltinType::Boolean)))),
            ("start", attr(BuiltinType::Number)),
            ("marker", hidden(attr(BuiltinType::String))),
        ]),
        transform: transform_fn(|node, config| {
            let head = list_head(node, config)?;
            let children = crate::next::transformer::children(node, config)?;
            let (name, attributes) = *head;
            Ok(tag_value(name, attributes, children))
        }),
        ..Default::default()
    }
}

/// The list's tag name (`ol` if `ordered` is truthy, else `ul`) and its transformed
/// attributes. (Apart, so that the frame the list's transform keeps on the stack while its
/// children transform stays small.)
#[inline(never)]
#[allow(clippy::type_complexity)]
fn list_head(
    node: &ResolvedNode<'_>,
    config: &Config,
) -> Result<Box<(&'static str, value::Object)>> {
    let ordered = node.attribute("ordered")?.to_boolean();
    Ok(Box::new((
        if ordered { "ol" } else { "ul" },
        node.transform_attributes(config)?,
    )))
}

/// `hr`.
pub fn hr() -> Schema {
    Schema {
        render: render("hr"),
        ..Default::default()
    }
}

/// `table`.
pub fn table() -> Schema {
    Schema {
        render: render("table"),
        ..Default::default()
    }
}

/// `td`.
pub fn td() -> Schema {
    Schema {
        render: render("td"),
        children: names(&[
            "inline",
            "heading",
            "paragraph",
            "image",
            "table",
            "tag",
            "fence",
            "blockquote",
            "list",
            "hr",
        ]),
        attributes: attrs(vec![
            ("align", attr(BuiltinType::String)),
            ("colspan", render_as(attr(BuiltinType::Number), "colSpan")),
            ("rowspan", render_as(attr(BuiltinType::Number), "rowSpan")),
        ]),
        ..Default::default()
    }
}

/// `th`.
pub fn th() -> Schema {
    Schema {
        render: render("th"),
        attributes: attrs(vec![
            ("width", attr(BuiltinType::String)),
            ("align", attr(BuiltinType::String)),
            ("colspan", render_as(attr(BuiltinType::Number), "colSpan")),
            ("rowspan", render_as(attr(BuiltinType::Number), "rowSpan")),
        ]),
        ..Default::default()
    }
}

/// `tr`.
pub fn tr() -> Schema {
    Schema {
        render: render("tr"),
        children: names(&["th", "td"]),
        ..Default::default()
    }
}

/// `tbody`.
pub fn tbody() -> Schema {
    Schema {
        render: render("tbody"),
        children: names(&["tr", "tag"]),
        ..Default::default()
    }
}

/// `thead`.
pub fn thead() -> Schema {
    Schema {
        render: render("thead"),
        children: names(&["tr"]),
        ..Default::default()
    }
}

/// `strong`.
pub fn strong() -> Schema {
    Schema {
        render: render("strong"),
        children: names(&["em", "s", "link", "code", "text", "tag"]),
        attributes: attrs(vec![("marker", hidden(attr(BuiltinType::String)))]),
        ..Default::default()
    }
}

/// `em`.
pub fn em() -> Schema {
    Schema {
        render: render("em"),
        children: names(&["strong", "s", "link", "code", "text", "tag"]),
        attributes: attrs(vec![("marker", hidden(attr(BuiltinType::String)))]),
        ..Default::default()
    }
}

/// `s`.
pub fn s() -> Schema {
    Schema {
        render: render("s"),
        children: names(&["strong", "em", "link", "code", "text", "tag"]),
        ..Default::default()
    }
}

/// `inline`.
pub fn inline() -> Schema {
    Schema {
        children: names(&[
            "strong",
            "em",
            "s",
            "code",
            "text",
            "tag",
            "link",
            "image",
            "hardbreak",
            "softbreak",
            "comment",
        ]),
        ..Default::default()
    }
}

/// `link`.
pub fn link() -> Schema {
    Schema {
        render: render("a"),
        children: names(&["strong", "em", "s", "code", "text", "tag"]),
        attributes: attrs(vec![
            ("href", required(attr(BuiltinType::String))),
            ("title", attr(BuiltinType::String)),
        ]),
        ..Default::default()
    }
}

/// `code`: `code` with the content as its child.
pub fn code() -> Schema {
    Schema {
        render: render("code"),
        attributes: attrs(vec![(
            "content",
            required(hidden(attr(BuiltinType::String))),
        )]),
        transform: transform_fn(|node, config| {
            let attributes = node.transform_attributes(config)?;
            let content = node.attribute("content")?;
            Ok(TagValue::new("code", attributes, vec![content]).into())
        }),
        ..Default::default()
    }
}

/// `text`: its content.
pub fn text() -> Schema {
    Schema {
        attributes: attrs(vec![("content", required(attr(BuiltinType::String)))]),
        transform: transform_fn(|node, _config| node.attribute("content")),
        ..Default::default()
    }
}

/// `hardbreak`.
pub fn hardbreak() -> Schema {
    Schema {
        render: render("br"),
        ..Default::default()
    }
}

/// `softbreak`: a space.
pub fn softbreak() -> Schema {
    Schema {
        transform: transform_fn(|_node, _config| Ok(Value::string(" "))),
        ..Default::default()
    }
}

/// `comment`.
pub fn comment() -> Schema {
    Schema {
        attributes: attrs(vec![("content", required(attr(BuiltinType::String)))]),
        ..Default::default()
    }
}

/// `error`.
pub fn error_schema() -> Schema {
    Schema::default()
}

/// `node`.
pub fn node() -> Schema {
    Schema::default()
}

static DEFAULT_NODES: LazyLock<IndexMap<String, Option<Schema>>> = LazyLock::new(|| {
    // In the order upstream's bundle exports them.
    [
        ("blockquote", blockquote()),
        ("code", code()),
        ("comment", comment()),
        ("document", document()),
        ("em", em()),
        ("error", error_schema()),
        ("fence", fence()),
        ("hardbreak", hardbreak()),
        ("heading", heading()),
        ("hr", hr()),
        ("image", image()),
        ("inline", inline()),
        ("item", item()),
        ("link", link()),
        ("list", list()),
        ("node", node()),
        ("paragraph", paragraph()),
        ("s", s()),
        ("softbreak", softbreak()),
        ("strong", strong()),
        ("table", table()),
        ("tbody", tbody()),
        ("td", td()),
        ("text", text()),
        ("th", th()),
        ("thead", thead()),
        ("tr", tr()),
    ]
    .into_iter()
    .map(|(name, schema)| (name.to_string(), Some(schema)))
    .collect()
});

/// Upstream's `nodes` (`import * as nodes from './src/schema'`).
pub fn default_nodes() -> &'static IndexMap<String, Option<Schema>> {
    &DEFAULT_NODES
}

/// `globalAttributes` (transformer.ts): `class` and `id`, on every node and tag.
pub fn global_attributes() -> &'static IndexMap<String, SchemaAttribute> {
    static GLOBAL: LazyLock<IndexMap<String, SchemaAttribute>> = LazyLock::new(|| {
        let mut out = IndexMap::new();
        out.insert(
            "class".to_string(),
            SchemaAttribute {
                attr_type: Some(TypeSpec::Markdoc(MarkdocType::Class)),
                render: Some(Value::Boolean(true)),
                ..Default::default()
            },
        );
        out.insert(
            "id".to_string(),
            SchemaAttribute {
                attr_type: Some(TypeSpec::Markdoc(MarkdocType::Id)),
                render: Some(Value::Boolean(true)),
                ..Default::default()
            },
        );
        out
    });
    &GLOBAL
}

/// `{...globalAttributes, ...schema.attributes}`: the attributes a node of this schema may
/// have, in a JS object's order.
pub fn merged_attributes(schema: Option<&Schema>) -> IndexMap<String, SchemaAttribute> {
    let mut out = global_attributes().clone();
    if let Some(attributes) = schema.and_then(|s| s.attributes.as_ref()) {
        for (k, v) in attributes {
            crate::types::js_insert(&mut out, k.clone(), v.clone());
        }
    }
    out
}

impl Config {
    /// An empty config (`{}`).
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_reads_loose_equality() {
        let a = |r: Option<Value>| SchemaAttribute {
            render: r,
            ..Default::default()
        };
        let renders =
            |attr: &SchemaAttribute, key| renders(attr, key).unwrap().map(Cow::into_owned);
        assert_eq!(renders(&a(None), "k").as_deref(), Some("k"));
        assert_eq!(
            renders(&a(Some(Value::Boolean(true))), "k").as_deref(),
            Some("k")
        );
        assert_eq!(
            renders(&a(Some(Value::string("x"))), "k").as_deref(),
            Some("x")
        );
        for hidden in [
            Value::Boolean(false),
            Value::Number(0.0),
            Value::string(""),
            Value::string("0"),
            Value::string(" 0x0 "),
            Value::array(vec![]),
        ] {
            assert_eq!(renders(&a(Some(hidden.clone())), "k"), None, "{hidden:?}");
        }
        assert_eq!(renders(&a(Some(Value::Null)), "k").as_deref(), Some("k"));
        let slot = SchemaSlot {
            render: Some(Value::string("")),
            ..SchemaSlot::default()
        };
        assert_eq!(slot.renders("k").unwrap().as_deref(), Some(""));
    }

    #[test]
    fn class_transform_joins_truthy_keys() {
        let mut o = IndexMap::new();
        o.insert("a".to_string(), Value::Boolean(true));
        o.insert("b".to_string(), Value::Number(0.0));
        o.insert("c".to_string(), Value::string("x"));
        assert_eq!(class_transform(Value::object(o)).as_str(), Some("a c"));
        assert!(matches!(
            class_transform(Value::Number(0.0)),
            Value::Number(_)
        ));
        assert_eq!(class_transform(Value::Number(5.0)).as_str(), Some(""));
        assert_eq!(
            class_transform(Value::array(vec![Value::Boolean(true), Value::Null])).as_str(),
            Some("0")
        );
    }

    #[test]
    fn a_config_can_be_shared_between_threads() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<Config>();
        send_sync::<Value>();
    }

    #[test]
    fn inherited_names_are_found() {
        let map: IndexMap<String, i32> = IndexMap::new();
        assert!(matches!(lookup(&map, "toString"), Found::Inherited));
        assert!(matches!(lookup(&map, "foo"), Found::Missing));
        assert!(!is_undefined_in(&map, "constructor"));
    }
}
