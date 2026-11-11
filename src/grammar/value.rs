//! The values the tag grammar builds: what tag.pegjs's `Value` rule returns, as the
//! JavaScript objects it creates behave.
//!
//! Hash literals and function parameters are JavaScript objects built with `{[key]: value}`,
//! `Object.assign` and `parameters[key] = value`, so [`Object`] keeps what those operations
//! keep: own properties in JavaScript's property order (array indices first, ascending, then
//! the other keys in creation order) and the `[[Prototype]]` that a `__proto__` key can
//! replace. Variables and functions are upstream's `Ast.Variable` and `Ast.Function`
//! instances (`{$$mdtype: "Variable", path}`, `{$$mdtype: "Function", name, parameters}`).

use std::collections::HashMap;

use serde_json::Map;

use super::json::{number_to_json, write_number, write_str};

/// A value of the tag grammar (`Value`, `VariableTail` and `@` paths in tag.pegjs).
///
/// A value nests as deep as the grammar nests it ([`crate::grammar::cost`]: node's limits, in
/// the thousands): `Clone` and `PartialEq` are `#[derive]`'s a level at a time on a stack with
/// room for it ([`crate::depth`]), and `Debug` writes 400 levels and `..` for what is
/// deeper.
pub enum Value {
    /// `null`
    Null,
    /// `true`, `false`
    Bool(bool),
    /// A number literal, `parseFloat` of its text (so `-0` is -0.0 and an overlong one is
    /// infinite).
    Number(f64),
    /// A string literal, escapes resolved.
    String(String),
    /// An array literal, or the path of an `@` variable (`@a.b` is `["a", "b"]`).
    Array(Vec<Value>),
    /// A hash literal.
    Object(Object),
    /// A `$` variable: `new Variable(path)`.
    Variable(Variable),
    /// A function call: `new Function(name, parameters)`.
    Function(Function),
}

impl Clone for Value {
    fn clone(&self) -> Self {
        crate::depth::grow!(match self {
            Value::Null => Value::Null,
            Value::Bool(b) => Value::Bool(*b),
            Value::Number(n) => Value::Number(*n),
            Value::String(s) => Value::String(s.clone()),
            Value::Array(items) => Value::Array(items.clone()),
            Value::Object(object) => Value::Object(object.clone()),
            Value::Variable(variable) => Value::Variable(variable.clone()),
            Value::Function(function) => Value::Function(function.clone()),
        })
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        crate::depth::grow!(match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => a == b,
            (Value::Object(a), Value::Object(b)) => a == b,
            (Value::Variable(a), Value::Variable(b)) => a == b,
            (Value::Function(a), Value::Function(b)) => a == b,
            _ => false,
        })
    }
}

impl std::fmt::Debug for Value {
    /// As `#[derive(Debug)]` writes it, down to 400 levels.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::types::debug_level(f, |f| match self {
            Value::Null => f.write_str("Null"),
            Value::Bool(b) => f.debug_tuple("Bool").field(b).finish(),
            Value::Number(n) => f.debug_tuple("Number").field(n).finish(),
            Value::String(s) => f.debug_tuple("String").field(s).finish(),
            Value::Array(items) => f.debug_tuple("Array").field(items).finish(),
            Value::Object(object) => f.debug_tuple("Object").field(object).finish(),
            Value::Variable(variable) => f.debug_tuple("Variable").field(variable).finish(),
            Value::Function(function) => f.debug_tuple("Function").field(function).finish(),
        })
    }
}

/// `new Variable([head, ...tail])`: upstream's `Ast.Variable`, `{$$mdtype: "Variable", path}`.
#[derive(Clone, Debug, PartialEq)]
pub struct Variable {
    /// The identifier, then each `.name` (a string) and `[index]` (a number or string).
    pub path: Vec<Value>,
}

/// `new Function(name, parameters)`: upstream's `Ast.Function`,
/// `{$$mdtype: "Function", name, parameters}`.
#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub name: String,
    /// Keyed by parameter name, or by position for a positional parameter.
    pub parameters: Object,
}

/// A JavaScript ordinary object, as a hash literal or a function's parameters build it.
#[derive(Clone, Debug, Default)]
pub struct Object {
    /// Own data properties, in creation order (see [`Object::entries`] for property order).
    props: Vec<(String, Value)>,
    /// Key to position in `props`, once there are more than [`INDEXED_FROM`] properties (as
    /// V8 turns a large object into a dictionary): `[[Set]]` stays O(1), so a hash literal
    /// with many keys takes linear time, not quadratic.
    index: Option<HashMap<String, usize>>,
    proto: Prototype,
}

const INDEXED_FROM: usize = 16;

impl PartialEq for Object {
    fn eq(&self, other: &Object) -> bool {
        self.props == other.props && self.proto == other.proto
    }
}

/// An [`Object`]'s `[[Prototype]]`. It is `Object.prototype` unless a `__proto__` key set
/// it: assigning `__proto__` to an object that inherits `Object.prototype`'s `__proto__`
/// accessor calls that accessor's setter instead of creating a property.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Prototype {
    /// `Object.prototype`
    #[default]
    ObjectPrototype,
    /// `null`
    Null,
    /// Another value: an object, an array (which inherits `Array.prototype`), a variable or a
    /// function (which inherit their class's prototype); all lead to `Object.prototype`
    /// unless an object on the way has a `null` prototype.
    Value(Box<Value>),
}

impl Value {
    /// JavaScript's `ToBoolean`, which `if (primary)` in tag.pegjs's `TagOpen` applies.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => !(*n == 0.0 || n.is_nan()),
            Value::String(s) => !s.is_empty(),
            Value::Array(_) | Value::Object(_) | Value::Variable(_) | Value::Function(_) => true,
        }
    }

    /// True for the values that are JavaScript objects (what `typeof v === "object"` and
    /// `v !== null` select).
    fn is_object(&self) -> bool {
        matches!(
            self,
            Value::Array(_) | Value::Object(_) | Value::Variable(_) | Value::Function(_)
        )
    }

    /// `JSON.parse(JSON.stringify(value))` as a `serde_json::Value`.
    ///
    /// Numbers: a non-finite one is `null`; one whose JavaScript text is a plain integer
    /// that fits `i64` or `u64` (every safe integer, -0 as 0, and larger whole numbers that
    /// JavaScript writes in full, such as 1152921504606847000) is held as that integer, so
    /// serde_json writes the same text; any other is held as the f64, which serde_json spells
    /// its own way (`1e21` for JavaScript's `1e+21`) but reads back as the same number.
    /// [`Value::to_json_string`] writes JavaScript's text exactly.
    ///
    /// Object keys are inserted in JavaScript's property order, which a `serde_json::Map`
    /// keeps only when serde_json's `preserve_order` feature is on (it sorts them otherwise).
    ///
    /// A value nests as deep as the grammar nests it ([`crate::grammar::cost`]); each level
    /// runs where the stack has room for it ([`crate::depth`]).
    pub fn to_json(&self) -> serde_json::Value {
        crate::depth::grow!(self.to_json_here())
    }

    #[inline(always)]
    fn to_json_here(&self) -> serde_json::Value {
        match self {
            Value::Null => serde_json::Value::Null,
            Value::Bool(b) => serde_json::Value::Bool(*b),
            Value::Number(n) => number_to_json(*n),
            Value::String(s) => serde_json::Value::String(s.clone()),
            Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(Value::to_json).collect())
            }
            Value::Object(o) => o.to_json(),
            Value::Variable(v) => {
                let mut m = Map::new();
                m.insert("$$mdtype".into(), "Variable".into());
                m.insert(
                    "path".into(),
                    serde_json::Value::Array(v.path.iter().map(Value::to_json).collect()),
                );
                serde_json::Value::Object(m)
            }
            Value::Function(f) => {
                let mut m = Map::new();
                m.insert("$$mdtype".into(), "Function".into());
                m.insert("name".into(), f.name.clone().into());
                m.insert("parameters".into(), f.parameters.to_json());
                serde_json::Value::Object(m)
            }
        }
    }

    /// `JSON.stringify(value)`, byte for byte.
    pub fn to_json_string(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out);
        out
    }

    pub(crate) fn write_json(&self, out: &mut String) {
        crate::depth::grow!(self.write_json_here(out));
    }

    #[inline(always)]
    fn write_json_here(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => write_number(out, *n),
            Value::String(s) => write_str(out, s),
            Value::Array(items) => write_array(out, items),
            Value::Object(o) => o.write_json(out),
            Value::Variable(v) => {
                out.push_str("{\"$$mdtype\":\"Variable\",\"path\":");
                write_array(out, &v.path);
                out.push('}');
            }
            Value::Function(f) => {
                out.push_str("{\"$$mdtype\":\"Function\",\"name\":");
                write_str(out, &f.name);
                out.push_str(",\"parameters\":");
                f.parameters.write_json(out);
                out.push('}');
            }
        }
    }
}

fn write_array(out: &mut String, items: &[Value]) {
    out.push('[');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        item.write_json(out);
    }
    out.push(']');
}

/// The array index a property key denotes, if any: a key P with
/// `ToString(ToUint32(P)) === P` and `ToUint32(P) !== 2^32 - 1` (ECMA-262 §6.1.7). Such
/// keys come first in an object's property order, in ascending numeric order.
fn array_index(key: &str) -> Option<u32> {
    let bytes = key.as_bytes();
    if bytes.is_empty() || bytes.len() > 10 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if bytes.len() > 1 && bytes[0] == b'0' {
        return None;
    }
    let n: u64 = key.parse().ok()?;
    (n < u64::from(u32::MAX)).then_some(n as u32)
}

impl Object {
    /// `{}`
    pub fn new() -> Object {
        Object::default()
    }

    /// `{[key]: value}`: a computed key creates an own property, even `__proto__`.
    pub(crate) fn with_property(key: String, value: Value) -> Object {
        let mut object = Object::new();
        object.create_data_property(key, value);
        object
    }

    /// The position of an own property in `props`.
    fn position(&self, key: &str) -> Option<usize> {
        match &self.index {
            Some(index) => index.get(key).copied(),
            None => self.props.iter().position(|(k, _)| k == key),
        }
    }

    /// CreateDataProperty for a key the object does not have yet.
    fn create_data_property(&mut self, key: String, value: Value) {
        if let Some(index) = &mut self.index {
            index.insert(key.clone(), self.props.len());
        } else if self.props.len() >= INDEXED_FROM {
            let mut index: HashMap<String, usize> = self
                .props
                .iter()
                .enumerate()
                .map(|(i, (k, _))| (k.clone(), i))
                .collect();
            index.insert(key.clone(), self.props.len());
            self.index = Some(index);
        }
        self.props.push((key, value));
    }

    /// `object[key] = value`: JavaScript's `[[Set]]` (OrdinarySet with the object as receiver).
    ///
    /// An own property is overwritten in place. Otherwise the prototype chain decides: every
    /// property an object built by the grammar can inherit is a writable data property
    /// (which makes `[[Set]]` create an own property) except `Object.prototype.__proto__`, an
    /// accessor whose setter replaces the `[[Prototype]]` with an object or `null` and
    /// ignores any other value. An own `__proto__` property of an object on the chain, or a
    /// chain ending in `null`, hides that accessor, and `__proto__` becomes an own property.
    pub fn set(&mut self, key: String, value: Value) {
        if let Some(i) = self.position(&key) {
            self.props[i].1 = value;
            return;
        }
        if key == "__proto__" && inherits_proto_accessor(&self.proto) {
            // `set Object.prototype.__proto__(proto)`: an object or null becomes the
            // prototype (the grammar cannot build a cycle); anything else is ignored.
            if let Value::Null = value {
                self.proto = Prototype::Null;
            } else if value.is_object() {
                self.proto = Prototype::Value(Box::new(value));
            }
            return;
        }
        self.create_data_property(key, value);
    }

    /// `Object.assign(this, source)`: `[[Set]]` of each of `source`'s own properties, in
    /// property order.
    pub(crate) fn assign(&mut self, source: Object) {
        let order = source.order();
        let mut props: Vec<Option<(String, Value)>> = source.props.into_iter().map(Some).collect();
        for i in order {
            let (key, value) = props[i].take().expect("each property is visited once");
            self.set(key, value);
        }
    }

    /// Indices into `props` in JavaScript's property order (`[[OwnPropertyKeys]]`, which
    /// `JSON.stringify`, `Object.keys` and `Object.assign` follow): array indices ascending,
    /// then the other keys in creation order.
    fn order(&self) -> Vec<usize> {
        let mut indices: Vec<(u32, usize)> = self
            .props
            .iter()
            .enumerate()
            .filter_map(|(i, (k, _))| array_index(k).map(|n| (n, i)))
            .collect();
        indices.sort_unstable();
        let mut order: Vec<usize> = indices.into_iter().map(|(_, i)| i).collect();
        order.extend((0..self.props.len()).filter(|&i| array_index(&self.props[i].0).is_none()));
        order
    }

    /// The own properties, in JavaScript's property order.
    pub fn entries(&self) -> Vec<(&str, &Value)> {
        self.order()
            .into_iter()
            .map(|i| (self.props[i].0.as_str(), &self.props[i].1))
            .collect()
    }

    /// An own property's value.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.position(key).map(|i| &self.props[i].1)
    }

    pub fn len(&self) -> usize {
        self.props.len()
    }

    pub fn is_empty(&self) -> bool {
        self.props.is_empty()
    }

    /// The `[[Prototype]]`.
    pub fn prototype(&self) -> &Prototype {
        &self.proto
    }

    /// `JSON.parse(JSON.stringify(object))`: the own properties only (see [`Value::to_json`]).
    pub fn to_json(&self) -> serde_json::Value {
        let mut m = Map::new();
        for (k, v) in self.entries() {
            m.insert(k.to_string(), v.to_json());
        }
        serde_json::Value::Object(m)
    }

    pub(crate) fn write_json(&self, out: &mut String) {
        out.push('{');
        for (i, (k, v)) in self.entries().into_iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            write_str(out, k);
            out.push(':');
            v.write_json(out);
        }
        out.push('}');
    }
}

/// Whether `[[Set]]` of `__proto__` on an object with this prototype reaches
/// `Object.prototype.__proto__` (the accessor) rather than an own `__proto__` data property
/// of an object on the chain or the chain's end.
fn inherits_proto_accessor(mut proto: &Prototype) -> bool {
    loop {
        match proto {
            Prototype::ObjectPrototype => return true,
            Prototype::Null => return false,
            Prototype::Value(value) => match &**value {
                Value::Object(o) => {
                    if o.get("__proto__").is_some() {
                        return false;
                    }
                    proto = &o.proto;
                }
                // Array.prototype, Variable.prototype and Function.prototype have no
                // `__proto__` of their own and inherit Object.prototype.
                _ => return true,
            },
        }
    }
}
