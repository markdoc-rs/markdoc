//! The JavaScript operations upstream's code applies to values, each with ECMAScript's
//! semantics over the object model of [`super`]: property reads through prototype chains,
//! `in`, `Object.entries`, `[[Set]]`, `ToPrimitive`, `ToString`, `ToNumber`,
//! `Array.prototype.join`, `JSON.stringify`, `===`, SameValueZero, `typeof`, `Array.isArray`,
//! and the checks a built-in array method makes before it runs (`map` being a function,
//! `ArraySpeciesCreate`).
//!
//! Owner: the model agent (A2).
//!
//! Every chain ends in a built-in object of the realm ([`crate::next::realm`], through
//! [`realm`]). Two kinds of reads go there:
//!
//! - what a document reads ([`get`]: `$x.key`, `obj[key]`, `node.attributes.primary`):
//!   [`realm::get_inherited`], the built-in's value ([`Value::Builtin`] for a function or an
//!   object), a getter run with the receiver;
//! - what an internal operation looks up to call or to compare ([`lookup`]: `toString` and
//!   `valueOf` for `ToPrimitive`, `join` for `Array.prototype.toString`, `map` and
//!   `constructor` for `ArraySpeciesCreate`, `resolve` for upstream's `resolve`, `toJSON`):
//!   the [`Intrinsic`] the realm names by identity. The conversions of the object model
//!   (`Object.prototype.toString`, `Array.prototype.join`, the wrappers' `toString` and
//!   `valueOf`, upstream's AST classes' `resolve`) are carried out here for a document's
//!   value; every other function, and every call whose `this` is itself a built-in, is
//!   the realm's `call` (a document's object can hold any function of the realm:
//!   `{toString: $s.trim}`).

use super::realm::{self, BuiltinProto, Ctor, Intrinsic, Method};
use super::{
    INVALID_STRING_LENGTH, JsString, JsStringBuilder, MAX_STRING_LENGTH, Object, Proto,
    RANGE_ERROR, Value,
};
use crate::markdown_it::js;
use crate::next::realm::{Builtin, Called, Wrapped};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

/// What JS throws converting an object to a primitive when neither its `toString` nor its
/// `valueOf` is a function that gives one (`OrdinaryToPrimitive`).
pub const NO_PRIMITIVE: &str = "TypeError: Cannot convert object to primitive value";

/// `TypeError: Cannot convert undefined or null to object` (`ToObject`).
pub const NOT_AN_OBJECT: &str = "TypeError: Cannot convert undefined or null to object";

// ---------------------------------------------------------------------------------------
// Types and prototypes
// ---------------------------------------------------------------------------------------

/// `typeof value`.
pub fn type_of(value: &Value) -> &'static str {
    match value {
        Value::Undefined => "undefined",
        Value::Null | Value::Array(_) | Value::Object(_) | Value::Tag(_) => "object",
        Value::Builtin(b) if b.is_callable() => "function",
        Value::Builtin(_) => "object",
        Value::Boolean(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
    }
}

/// `Array.isArray(value)`.
pub fn is_array(value: &Value) -> bool {
    match value {
        Value::Array(_) => true,
        // `Array.prototype` is an array.
        Value::Builtin(b) => b.is_array(),
        _ => false,
    }
}

/// `typeof value === 'function'`: a function of the realm (no value a document makes is
/// one).
pub fn is_callable(value: &Value) -> bool {
    matches!(value, Value::Builtin(b) if b.is_callable())
}

/// `Object.getPrototypeOf(Object(value))`: `None` for `null` and `undefined`, which have no
/// object.
pub fn prototype_of(value: &Value) -> Option<Proto> {
    Some(match value {
        Value::Undefined | Value::Null => return None,
        Value::Boolean(_) => Proto::Builtin(BuiltinProto::BOOLEAN_PROTOTYPE),
        Value::Number(_) => Proto::Builtin(BuiltinProto::NUMBER_PROTOTYPE),
        Value::String(_) => Proto::Builtin(BuiltinProto::STRING_PROTOTYPE),
        Value::Array(_) => Proto::Builtin(BuiltinProto::ARRAY_PROTOTYPE),
        Value::Object(o) => o.proto().clone(),
        Value::Tag(_) => Proto::Builtin(BuiltinProto::TAG_PROTOTYPE),
        Value::Builtin(b) => match b.proto() {
            Some(p) => Proto::Builtin(p),
            None => Proto::Null,
        },
    })
}

/// Is the value's prototype `Object.prototype` (`Object.getPrototypeOf(value) ===
/// Object.prototype`)? `false` for `null` and `undefined` (where upstream asks, they are
/// handled before).
pub fn has_object_prototype(value: &Value) -> bool {
    matches!(value, Value::Object(o) if o.proto().is_object_prototype())
}

/// The index a key names, when it is a canonical array index.
fn index_of(key: &str) -> Option<u32> {
    crate::types::array_index(key)
}

/// `value`'s own property `key` (of `Object(value)` for a primitive), if it has one.
pub fn own_property(value: &Value, key: &str) -> Option<Value> {
    match value {
        Value::Undefined | Value::Null | Value::Boolean(_) | Value::Number(_) => None,
        Value::String(s) => {
            if key == "length" {
                return Some(Value::Number(s.len_utf16() as f64));
            }
            let i = index_of(key)? as usize;
            s.code_units()
                .nth(i)
                .map(|unit| Value::String(JsString::from_code_unit(unit)))
        }
        Value::Array(items) => {
            if key == "length" {
                return Some(Value::Number(items.len() as f64));
            }
            match index_of(key) {
                Some(i) => items.get(i as usize).cloned(),
                None => items.named_get(key).cloned(),
            }
        }
        Value::Object(o) => o.get(key).cloned(),
        Value::Tag(tag) => match key {
            "$$mdtype" => Some(Value::string("Tag")),
            "name" => Some(tag.name.clone()),
            "attributes" => Some(Value::Object(Arc::new(tag.attributes.clone()))),
            "children" => Some(Value::array(tag.children.clone())),
            _ => None,
        },
        // A built-in's own data property (its accessors are read with the receiver: see
        // [`lookup`], which reads every built-in through the realm).
        Value::Builtin(b) => match &b.own(key)?.slot {
            crate::next::realm::Slot::Data(d) => Some(d.to_value()),
            crate::next::realm::Slot::Accessor { .. } => None,
        },
    }
}

/// Does `value` have the own property `key`?
fn has_own(value: &Value, key: &str) -> bool {
    match value {
        Value::Undefined | Value::Null | Value::Boolean(_) | Value::Number(_) => false,
        Value::String(s) => {
            key == "length" || index_of(key).is_some_and(|i| (i as usize) < s.len_utf16())
        }
        Value::Array(items) => {
            key == "length"
                || match index_of(key) {
                    Some(i) => items.get(i as usize).is_some(),
                    None => items.named_get(key).is_some(),
                }
        }
        Value::Object(o) => o.contains_key(key),
        Value::Tag(_) => matches!(key, "$$mdtype" | "name" | "attributes" | "children"),
        Value::Builtin(b) => b.own(key).is_some(),
    }
}

/// What `value[key]` finds walking the prototype chain: a property a document's object holds
/// (the value itself or an object on its chain), or, once the chain reaches one, a built-in
/// prototype with what the realm names there.
#[derive(Debug, Clone)]
pub enum Found {
    Value(Value),
    Builtin(BuiltinProto, Intrinsic),
}

impl Found {
    /// Is it `undefined`?
    pub fn is_undefined(&self) -> bool {
        matches!(
            self,
            Found::Value(Value::Undefined) | Found::Builtin(_, Intrinsic::Absent)
        )
    }

    /// Is it truthy (`!!value[key]`)? Every built-in the realm has is an object or a
    /// function (truthy).
    pub fn is_truthy(&self) -> bool {
        match self {
            Found::Value(v) => v.to_boolean(),
            Found::Builtin(_, i) => *i != Intrinsic::Absent,
        }
    }
}

/// `TypeError: Cannot read properties of undefined (reading 'key')`.
pub fn read_error(value: &Value, key: &str) -> String {
    format!(
        "TypeError: Cannot read properties of {} (reading '{key}')",
        if value.is_undefined() {
            "undefined"
        } else {
            "null"
        }
    )
}

/// `value[key]` as an internal operation reads it: the chain walked (own properties, then each
/// prototype's) until a property or a built-in prototype is found. `Err` (JS's `TypeError`)
/// for `null` and `undefined`.
pub fn lookup(value: &Value, key: &str) -> Result<Found, String> {
    if value.is_nullish() {
        return Err(read_error(value, key));
    }
    // A built-in's own properties and chain are the realm's.
    if let Value::Builtin(b) = value {
        return at_builtin(*b, key, value);
    }
    if let Some(v) = own_property(value, key) {
        return Ok(Found::Value(v));
    }
    let mut proto = prototype_of(value);
    loop {
        match proto {
            None | Some(Proto::Null) => return Ok(Found::Value(Value::Undefined)),
            Some(Proto::Builtin(p)) => return at_builtin(p, key, value),
            Some(Proto::Value(p)) => {
                if let Some(v) = own_property(&p, key) {
                    return Ok(Found::Value(v));
                }
                proto = prototype_of(&p);
            }
        }
    }
}

/// What [`lookup`] finds once the chain reaches the built-in `p` (`p` itself included): one of
/// the methods or constructors the operations here carry out or compare themselves
/// ([`Found::Builtin`]), `undefined`, or the realm's value (a getter run with `receiver` as
/// `this`, which may throw).
fn at_builtin(p: BuiltinProto, key: &str, receiver: &Value) -> Result<Found, String> {
    Ok(match realm::intrinsic(p, key) {
        i @ (Intrinsic::Method(_) | Intrinsic::Ctor(_) | Intrinsic::Absent) => Found::Builtin(p, i),
        Intrinsic::ProtoAccessor | Intrinsic::Other => {
            Found::Value(realm::get_inherited(p, key, receiver)?)
        }
    })
}

/// The function a property holds, as an internal operation calls it: a method the operations
/// here carry out themselves ([`Callee::Model`]; only for a `this` the object model holds),
/// any other function of the realm ([`Callee::Realm`]), or nothing callable.
pub(crate) enum Callee {
    Model(Method),
    Realm(Builtin),
    None,
}

/// What [`lookup`] found, as a callee for `this`.
pub(crate) fn callee(found: Found, this: &Value) -> Callee {
    let f = match found {
        Found::Builtin(_, Intrinsic::Method(m)) => m.builtin(),
        Found::Builtin(_, Intrinsic::Ctor(c)) => c.builtin(),
        Found::Builtin(..) => return Callee::None,
        Found::Value(Value::Builtin(f)) if f.is_callable() => f,
        Found::Value(_) => return Callee::None,
    };
    match Method::of(f) {
        Some(m) if !matches!(this, Value::Builtin(_)) => Callee::Model(m),
        _ => Callee::Realm(f),
    }
}

/// `value[key]` as a document reads it (`$x.key`, `obj[key]`): an own property, else the
/// prototype chain's; at a built-in prototype the realm's answer ([`realm::get_inherited`],
/// with `value` as the receiver an accessor's getter sees). `Err` for `null` and `undefined`
/// (JS's `TypeError`) and where a getter throws (`Function.prototype.caller`). A string's
/// index is its code unit there (half of a surrogate pair is that lone surrogate).
pub fn get(value: &Value, key: &str) -> Result<Value, String> {
    match lookup(value, key)? {
        Found::Value(v) => Ok(v),
        Found::Builtin(p, _) => realm::get_inherited(p, key, value),
    }
}

/// The former name of [`get`].
pub fn get_property(value: &Value, key: &str) -> Result<Value, String> {
    get(value, key)
}

/// `object[key]` for an object held as an [`Object`] (a resolved node's attributes): its own
/// property, else its prototype chain's (see [`get`]).
pub fn object_get(object: &Object, key: &str) -> Result<Value, String> {
    if let Some(v) = object.get(key) {
        return Ok(v.clone());
    }
    let mut proto = object.proto().clone();
    loop {
        match proto {
            Proto::Null => return Ok(Value::Undefined),
            Proto::Builtin(p) => {
                return match realm::intrinsic(p, key) {
                    Intrinsic::Absent => Ok(Value::Undefined),
                    // The receiver a getter sees: the object itself.
                    _ => realm::get_inherited(p, key, &Value::Object(Arc::new(object.clone()))),
                };
            }
            Proto::Value(v) => {
                if let Some(x) = own_property(&v, key) {
                    return Ok(x);
                }
                proto = prototype_of(&v).unwrap_or(Proto::Null);
            }
        }
    }
}

/// `object[key] === undefined` for an object held as an [`Object`].
pub fn object_is_undefined_at(object: &Object, key: &str) -> bool {
    if let Some(v) = object.get(key) {
        return v.is_undefined();
    }
    match object.proto() {
        Proto::Null => true,
        Proto::Builtin(p) => realm::intrinsic(*p, key) == Intrinsic::Absent,
        Proto::Value(v) => lookup(v, key).is_ok_and(|f| f.is_undefined()),
    }
}

/// `key in object` for an object held as an [`Object`].
pub fn object_has_property(object: &Object, key: &str) -> bool {
    if object.contains_key(key) {
        return true;
    }
    match object.proto() {
        Proto::Null => false,
        Proto::Builtin(p) => realm::has_inherited(*p, key),
        Proto::Value(v) => has_property(v, key).unwrap_or(false),
    }
}

/// The elements an array-like value has, as a built-in array method visits them (`reduce`,
/// `map`, `join`): `LengthOfArrayLike(value)`, then each index below it that `HasProperty`
/// finds (on the value or its chain), ascending, with what `Get` reads there. `Err` where
/// reading the length throws.
pub fn array_like_elements(value: &Value) -> Result<(u64, Vec<(u64, Value)>), String> {
    Ok(match value {
        // An array's prototype is always `Array.prototype`, which holds no element.
        Value::Array(items) => (
            items.len() as u64,
            items
                .present()
                .map(|(i, v)| (u64::from(i), v.clone()))
                .collect(),
        ),
        // `Array.prototype`: an array of none.
        Value::Builtin(b) if b.is_array() => (0, Vec::new()),
        _ => {
            let length = to_length(&get_value(value, "length")?)?.min(f64::from(u32::MAX)) as u64;
            (length, chain_elements(value, length))
        }
    })
}

/// `value[key] === undefined` (a built-in the chain finds is a function or an object, never
/// `undefined`). `Err` for `null` and `undefined`.
pub fn is_undefined_at(value: &Value, key: &str) -> Result<bool, String> {
    Ok(lookup(value, key)?.is_undefined())
}

/// `!!value[key]`. `Err` for `null` and `undefined`.
pub fn is_truthy_at(value: &Value, key: &str) -> Result<bool, String> {
    Ok(lookup(value, key)?.is_truthy())
}

/// `key in value` (`HasProperty`): an own property, or one an object on the chain or the
/// built-in at its end has. `Err` where JS's `in` throws: a primitive is no object.
pub fn has_property(value: &Value, key: &str) -> Result<bool, String> {
    if !value.is_object() {
        return Err(format!(
            "TypeError: Cannot use 'in' operator to search for '{key}' in {}",
            to_js_string(value)
                .map(JsString::into_string_lossy)
                .unwrap_or_default()
        ));
    }
    let mut at = value.clone();
    loop {
        if has_own(&at, key) {
            return Ok(true);
        }
        match prototype_of(&at) {
            None | Some(Proto::Null) => return Ok(false),
            Some(Proto::Builtin(p)) => return Ok(realm::has_inherited(p, key)),
            Some(Proto::Value(p)) => at = p,
        }
    }
}

/// `Object.prototype.hasOwnProperty.call(value, key)`: `TypeError` for `null` and
/// `undefined`, which have no object.
pub fn has_own_property(value: &Value, key: &str) -> Result<bool, String> {
    if value.is_nullish() {
        return Err(NOT_AN_OBJECT.into());
    }
    Ok(has_own(value, key))
}

/// `object[key] = value` (`OrdinarySet` with the object as the receiver) on an object nothing
/// else holds yet: an own property is overwritten in its place; otherwise the chain decides.
/// Every property an object can inherit here is a writable data property (which makes the
/// assignment create an own property) except `Object.prototype.__proto__`, an accessor whose
/// setter replaces the prototype with an object or `null` and ignores anything else.
pub fn set(object: &mut Object, key: String, value: Value) {
    if let Some(slot) = object.get_mut(&key) {
        *slot = value;
        return;
    }
    if inherits_setter(object.proto(), &key) {
        // `set Object.prototype.__proto__`.
        if let Some(proto) = Proto::from_value(&value) {
            object.set_proto(proto);
        }
        return;
    }
    object.insert(key, value);
}

/// Does `[[Set]]` of `key` on an object with this prototype reach an accessor (the only one a
/// chain here has: `Object.prototype.__proto__`) rather than an own data property of an
/// object on the chain or the chain's end?
pub fn inherits_setter(proto: &Proto, key: &str) -> bool {
    if key != "__proto__" {
        return false;
    }
    let mut at = proto.clone();
    loop {
        match at {
            Proto::Null => return false,
            Proto::Builtin(p) => return realm::intrinsic(p, key) == Intrinsic::ProtoAccessor,
            Proto::Value(v) => {
                if has_own(&v, key) {
                    return false;
                }
                at = prototype_of(&v).unwrap_or(Proto::Null);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Own properties
// ---------------------------------------------------------------------------------------

/// A value's own enumerable properties, in order: what `Object.entries(Object(value))` and
/// `{...value}` see (`null` and `undefined` have none here; see [`own_entries_of`]). A
/// string's code units and an array's present elements are keyed by index (an array's other
/// own properties follow; `length` is not enumerable); numbers and booleans have none; a
/// prototype adds nothing.
pub fn own_entries(value: &Value) -> Vec<(String, Value)> {
    match value {
        // A built-in object has the table's enumerable ones (only `Tag.isTag` is).
        Value::Builtin(b) => b
            .data()
            .props
            .iter()
            .filter(|p| p.attrs & crate::next::realm::ENUMERABLE != 0)
            .map(|p| {
                let v = match &p.slot {
                    crate::next::realm::Slot::Data(d) => d.to_value(),
                    // No enumerable accessor is in the table.
                    crate::next::realm::Slot::Accessor { .. } => Value::Undefined,
                };
                (p.key.to_string(), v)
            })
            .collect(),
        Value::Undefined | Value::Null | Value::Boolean(_) | Value::Number(_) => Vec::new(),
        Value::String(s) => s
            .code_units()
            .enumerate()
            .map(|(i, unit)| (i.to_string(), Value::String(JsString::from_code_unit(unit))))
            .collect(),
        Value::Array(items) => items
            .present()
            .map(|(i, v)| (i.to_string(), v.clone()))
            .chain(items.named().map(|(k, v)| (k.clone(), v.clone())))
            .collect(),
        Value::Object(o) => o.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        Value::Tag(tag) => vec![
            ("$$mdtype".into(), Value::string("Tag")),
            ("name".into(), tag.name.clone()),
            (
                "attributes".into(),
                Value::Object(Arc::new(tag.attributes.clone())),
            ),
            ("children".into(), Value::array(tag.children.clone())),
        ],
    }
}

/// `Object.entries(value)`: [`own_entries`], but `null` and `undefined` throw (`TypeError:
/// Cannot convert undefined or null to object`).
pub fn own_entries_of(value: &Value) -> Result<Vec<(String, Value)>, String> {
    if value.is_nullish() {
        return Err(NOT_AN_OBJECT.into());
    }
    Ok(own_entries(value))
}

/// `Object.values(value)` (`null` and `undefined` throw).
pub fn own_values_of(value: &Value) -> Result<Vec<Value>, String> {
    Ok(own_entries_of(value)?.into_iter().map(|(_, v)| v).collect())
}

// ---------------------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------------------

/// `ToPrimitive`'s hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    String,
    Number,
    /// No preference (`==`, `+`): for every object here, as `Number` (only a `Date` or a
    /// `Symbol.toPrimitive` differs, and no value here has either).
    Default,
}

/// What `ToPrimitive` of an object comes to before anything is joined: a primitive, or the
/// object's `Array.prototype.join` with `,` (its `toString` was `Array.prototype.toString`,
/// whose `this.join` was `Array.prototype.join`).
enum Plan {
    Primitive(Value),
    Join(Value),
}

/// `ToPrimitive(object, hint)` up to a join (`OrdinaryToPrimitive`: `toString` then `valueOf`
/// for the hint `string`, the other way round for `number`; the first that is callable and
/// gives a primitive wins).
fn primitive_plan(object: &Value, hint: Hint) -> Result<Plan, String> {
    let order: [&str; 2] = match hint {
        Hint::String => ["toString", "valueOf"],
        Hint::Number | Hint::Default => ["valueOf", "toString"],
    };
    for name in order {
        let callee = match callee(lookup(object, name)?, object) {
            // Not one of the conversions carried out here: the realm calls it.
            Callee::Model(method) if !is_conversion(method) => Callee::Realm(method.builtin()),
            other => other,
        };
        match callee {
            Callee::Model(method) => {
                if let Some(plan) = call_conversion(method, object)? {
                    return Ok(plan);
                }
            }
            // A function of the realm, called with the object as `this` and no argument.
            Callee::Realm(f) => {
                if let Some(primitive) = crate::next::realm::call(f, object, &[])?.primitive() {
                    return Ok(Plan::Primitive(primitive));
                }
            }
            Callee::None => {}
        }
    }
    Err(NO_PRIMITIVE.into())
}

/// Is it one of the methods [`call_conversion`] carries out (every built-in `toString` and
/// `valueOf` an object's chain has by default, and `join`)?
fn is_conversion(method: Method) -> bool {
    matches!(
        method,
        Method::ObjectToString
            | Method::ObjectValueOf
            | Method::ArrayToString
            | Method::ArrayJoin
            | Method::StringToString
            | Method::StringValueOf
            | Method::NumberToString
            | Method::NumberValueOf
            | Method::BooleanToString
            | Method::BooleanValueOf
    )
}

/// A built-in `toString` or `valueOf` called with an object as `this`: what it returns when
/// that is a primitive (or the join it comes to), `None` when it returns an object.
fn call_conversion(method: Method, object: &Value) -> Result<Option<Plan>, String> {
    Ok(Some(match method {
        Method::ObjectToString => Plan::Primitive(Value::string(object_to_string(object))),
        // Returns the object itself: no primitive.
        Method::ObjectValueOf => return Ok(None),
        // `Array.prototype.toString`: `this.join` if it is a function, else
        // `Object.prototype.toString`.
        Method::ArrayToString => match callee(lookup(object, "join")?, object) {
            Callee::Model(Method::ArrayJoin) => Plan::Join(object.clone()),
            // Another function: called with the object as `this`, its result whatever it is.
            Callee::Model(m) => match crate::next::realm::call(m.builtin(), object, &[])? {
                Called::Value(v) if !v.is_object() => Plan::Primitive(v),
                _ => return Ok(None),
            },
            Callee::Realm(f) => match crate::next::realm::call(f, object, &[])? {
                Called::Value(v) if !v.is_object() => Plan::Primitive(v),
                _ => return Ok(None),
            },
            Callee::None => Plan::Primitive(Value::string(object_to_string(object))),
        },
        Method::ArrayJoin => Plan::Join(object.clone()),
        Method::StringToString | Method::StringValueOf => {
            return Err(this_error("String", method_name(method)));
        }
        Method::NumberToString | Method::NumberValueOf => {
            return Err(this_error("Number", method_name(method)));
        }
        Method::BooleanToString | Method::BooleanValueOf => {
            return Err(this_error("Boolean", method_name(method)));
        }
        // No built-in's `toString` or `valueOf` is another method.
        _ => return Ok(None),
    }))
}

fn method_name(method: Method) -> &'static str {
    match method {
        Method::StringToString | Method::NumberToString | Method::BooleanToString => "toString",
        _ => "valueOf",
    }
}

/// `TypeError: String.prototype.toString requires that 'this' be a String`.
fn this_error(class: &str, method: &str) -> String {
    format!("TypeError: {class}.prototype.{method} requires that 'this' be a {class}")
}

/// `Object.prototype.toString.call(object)` of an object: `[object Array]` for an array,
/// `[object Object]` for any other (no value here has a `Symbol.toStringTag`).
fn object_to_string(object: &Value) -> &'static str {
    if is_array(object) {
        "[object Array]"
    } else {
        "[object Object]"
    }
}

/// `ToPrimitive(value, hint)`: a primitive as it is; an object through its `toString` and
/// `valueOf` (see [`to_js_string`]).
pub fn to_primitive(value: &Value, hint: Hint) -> Result<Value, String> {
    if !value.is_object() {
        return Ok(value.clone());
    }
    match primitive_plan(value, hint)? {
        Plan::Primitive(p) => Ok(p),
        Plan::Join(o) => Ok(Value::String(join_with(&o, &JsString::from(","))?)),
    }
}

/// `String(value)` (ECMAScript `ToString`): numbers as JS prints them, an object by
/// `ToPrimitive` with the hint `string` (its `toString`, else its `valueOf`, found through its
/// prototype chain: an array's is `join`, a plain object's `[object Object]`). `Err` where JS
/// throws: no callable `toString` or `valueOf` gives a primitive (`TypeError: Cannot convert
/// object to primitive value`), arrays nested deeper than [`super::MAX_JOIN_DEPTH`] (a
/// `RangeError`), a string past [`MAX_STRING_LENGTH`].
pub fn to_js_string(value: &Value) -> Result<JsString, String> {
    Ok(match value {
        Value::Undefined => "undefined".into(),
        Value::Null => "null".into(),
        Value::Boolean(b) => (if *b { "true" } else { "false" }).into(),
        Value::Number(n) => js::number_to_string(*n).into(),
        Value::String(s) => s.clone(),
        Value::Array(_) | Value::Object(_) | Value::Tag(_) | Value::Builtin(_) => {
            return match primitive_plan(value, Hint::String)? {
                Plan::Primitive(p) => to_js_string(&p),
                Plan::Join(o) => join_with(&o, &JsString::from(",")),
            };
        }
    })
}

/// `ToNumber(value)`.
pub fn to_number(value: &Value) -> Result<f64, String> {
    Ok(match value {
        Value::Undefined => f64::NAN,
        Value::Null => 0.0,
        Value::Boolean(b) => f64::from(u8::from(*b)),
        Value::Number(n) => *n,
        Value::String(s) => js::string_to_number(&s.to_utf16()),
        Value::Array(_) | Value::Object(_) | Value::Tag(_) | Value::Builtin(_) => {
            return to_number(&to_primitive(value, Hint::Number)?);
        }
    })
}

/// `ToPropertyKey(value)`: a string as it is, anything else `ToString`'d (an object by
/// `ToPrimitive` with the hint `string`). A lone surrogate becomes U+FFFD (keys are Rust
/// strings; see the module's divergences).
pub fn to_property_key(value: &Value) -> Result<String, String> {
    Ok(to_js_string(value)?.into_string_lossy())
}

/// `ToLength(value)` for a length read from an object: an integer from 0 to 2^53 - 1.
fn to_length(value: &Value) -> Result<f64, String> {
    let n = to_number(value)?;
    if n.is_nan() || n <= 0.0 {
        return Ok(0.0);
    }
    Ok(n.trunc().min(9_007_199_254_740_991.0))
}

// ---------------------------------------------------------------------------------------
// Array.prototype.join
// ---------------------------------------------------------------------------------------

/// `Array.prototype.join.call(value, separator)`: the elements of `value` (an array, or any
/// object read through its prototype chain: a plain object whose prototype is an array joins
/// that array's elements) from 0 to its `length`, `undefined` and `null` (and a hole) as
/// nothing, any other `ToString`'d, between them `separator` (`undefined` is `,`). V8's
/// errors: `TypeError: Invalid array length` for a length past 2^32 - 1 on an object that is
/// not an array; `RangeError: Invalid string length` for a result past [`MAX_STRING_LENGTH`]
/// code units, once every element is converted (V8 converts them all first: an element that
/// throws wins over the length); a `RangeError` past [`super::MAX_JOIN_DEPTH`] nested joins. A sparse
/// array costs the time of its present elements, whatever its length.
pub fn array_join(value: &Value, separator: &Value) -> Result<JsString, String> {
    let separator = match separator {
        Value::Undefined => JsString::from(","),
        other => to_js_string(other)?,
    };
    join_with(value, &separator)
}

/// The index keys below `length` that an array-like `value` or an object on its prototype
/// chain holds, ascending, each with the value found first along the chain (what `Get(O, k)`
/// gives for each `k` with `HasProperty(O, k)`).
fn chain_elements(value: &Value, length: u64) -> Vec<(u64, Value)> {
    let mut found: BTreeMap<u64, Value> = BTreeMap::new();
    let mut at = Some(value.clone());
    while let Some(v) = at.take() {
        match &v {
            Value::Array(items) => {
                for (i, x) in items.present() {
                    if u64::from(i) >= length {
                        break;
                    }
                    found.entry(u64::from(i)).or_insert_with(|| x.clone());
                }
            }
            Value::Object(o) => {
                for (k, x) in o.iter() {
                    match index_of(k) {
                        Some(i) if u64::from(i) < length => {
                            found.entry(u64::from(i)).or_insert_with(|| x.clone());
                        }
                        Some(_) => {}
                        // An object's index keys come first.
                        None => break,
                    }
                }
            }
            Value::String(s) => {
                for (i, unit) in s.code_units().enumerate() {
                    if i as u64 >= length {
                        break;
                    }
                    found
                        .entry(i as u64)
                        .or_insert_with(|| Value::String(JsString::from_code_unit(unit)));
                }
            }
            _ => {}
        }
        if let Some(Proto::Value(p)) = prototype_of(&v) {
            at = Some(p);
        }
    }
    found.into_iter().collect()
}

/// One array-like being joined: its elements present (index, value), how far the conversion
/// has got, and the strings made so far.
struct JoinFrame {
    elements: std::vec::IntoIter<(u64, Value)>,
    length: u64,
    pieces: Vec<(u64, JsString)>,
}

impl JoinFrame {
    fn new(value: &Value) -> Result<JoinFrame, String> {
        let (length, elements) = match value {
            // An array's prototype is always `Array.prototype`, which holds no element.
            Value::Array(items) => (
                items.len() as u64,
                items
                    .present()
                    .map(|(i, v)| (u64::from(i), v.clone()))
                    .collect::<Vec<_>>(),
            ),
            _ => {
                let length = to_length(&get_value(value, "length")?)?;
                if length > f64::from(u32::MAX) {
                    return Err("TypeError: Invalid array length".into());
                }
                let length = length as u64;
                (length, chain_elements(value, length))
            }
        };
        Ok(JoinFrame {
            elements: elements.into_iter(),
            length,
            pieces: Vec::new(),
        })
    }

    /// The joined string, once every element is converted: `length - 1` separators, each
    /// element's string after the separators before its index.
    fn finish(self, separator: &JsString) -> Result<JsString, String> {
        let separators = self.length.saturating_sub(1);
        let sep_units = separator.len_utf16() as u64;
        let units = self
            .pieces
            .iter()
            .map(|(_, s)| s.len_utf16() as u64)
            .sum::<u64>()
            + separators * sep_units;
        if units > MAX_STRING_LENGTH as u64 {
            return Err(INVALID_STRING_LENGTH.into());
        }
        let mut out = JsStringBuilder::new();
        let repeat = |out: &mut JsStringBuilder, n: u64| {
            if sep_units > 0 {
                for _ in 0..n {
                    out.push_js(separator);
                }
            }
        };
        // The separators written so far: those before indices 1 to `written`.
        let mut written = 0u64;
        for (i, piece) in &self.pieces {
            repeat(&mut out, i - written);
            written = *i;
            out.push_js(piece);
        }
        repeat(&mut out, separators - written.min(separators));
        Ok(out.finish())
    }
}

/// [`array_join`] with a separator already a string: a loop over a stack of the array-likes
/// being joined (at most [`super::MAX_JOIN_DEPTH`] from the top of a thread: as many as what is
/// left of the depth budget takes, [`super::JOIN_COST`] each).
fn join_with(value: &Value, separator: &JsString) -> Result<JsString, String> {
    let limit = usize::try_from(crate::depth::left() / super::JOIN_COST).unwrap_or(usize::MAX);
    if limit == 0 {
        return Err(RANGE_ERROR.into());
    }
    let mut stack = vec![JoinFrame::new(value)?];
    loop {
        let next = stack.last_mut().expect("a frame").elements.next();
        let Some((i, element)) = next else {
            let frame = stack.pop().expect("a frame");
            let s = frame.finish(separator)?;
            match stack.last_mut() {
                None => return Ok(s),
                // The string of the element whose conversion pushed this frame.
                Some(parent) => parent.pieces.last_mut().expect("its placeholder").1 = s,
            }
            continue;
        };
        let piece = match &element {
            Value::Undefined | Value::Null => JsString::new(),
            Value::Array(_) | Value::Object(_) | Value::Tag(_) | Value::Builtin(_) => {
                match primitive_plan(&element, Hint::String)? {
                    Plan::Primitive(p) => to_js_string(&p)?,
                    Plan::Join(o) => {
                        if stack.len() >= limit {
                            return Err(RANGE_ERROR.into());
                        }
                        let inner = JoinFrame::new(&o)?;
                        // A placeholder, which the inner join's string replaces.
                        stack
                            .last_mut()
                            .expect("a frame")
                            .pieces
                            .push((i, JsString::new()));
                        stack.push(inner);
                        continue;
                    }
                }
            }
            other => to_js_string(other)?,
        };
        stack.last_mut().expect("a frame").pieces.push((i, piece));
    }
}

/// `value[key]` where an internal operation reads a value (a length, an element): [`get`].
pub(crate) fn get_value(value: &Value, key: &str) -> Result<Value, String> {
    get(value, key)
}

// ---------------------------------------------------------------------------------------
// Equality, truthiness
// ---------------------------------------------------------------------------------------

/// `a === b`: primitives by value (`NaN` is not itself, `0` is `-0`, strings code unit by
/// code unit), objects, arrays and tags by identity.
pub fn strict_equals(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
        (Value::Boolean(x), Value::Boolean(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => x == y,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => Arc::ptr_eq(x, y),
        (Value::Object(x), Value::Object(y)) => Arc::ptr_eq(x, y),
        (Value::Tag(x), Value::Tag(y)) => Arc::ptr_eq(x, y),
        (Value::Builtin(x), Value::Builtin(y)) => x == y,
        _ => false,
    }
}

/// SameValueZero (`Array.prototype.includes`, `Set.prototype.has`): `===`, except that
/// `NaN` is itself.
pub fn same_value_zero(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) if x.is_nan() && y.is_nan() => true,
        _ => strict_equals(a, b),
    }
}

/// Markdoc's `truthy` (`src/tags/conditional.ts`): anything but `false`, `undefined` and
/// `null`. Not JS's truthiness: `0` and `''` are truthy here.
pub fn truthy(value: &Value) -> bool {
    !matches!(
        value,
        Value::Boolean(false) | Value::Undefined | Value::Null
    )
}

// ---------------------------------------------------------------------------------------
// Built-in methods' checks
// ---------------------------------------------------------------------------------------

/// `ArraySpeciesCreate(array, n)`'s errors, which upstream meets where it calls `map` or
/// `flatMap` on an array: the array's `constructor` (own, else `Array`) must be `undefined`,
/// a constructor, or an object (whose `Symbol.species`, which no document object has, is
/// `undefined`); anything else is `TypeError: object.constructor[Symbol.species] is not a
/// constructor` (V8's message). The new array is a plain one in every case that does not
/// throw.
pub fn species_check(array: &Value) -> Result<(), String> {
    if !is_array(array) {
        return Ok(());
    }
    match lookup(array, "constructor")? {
        Found::Builtin(..) => Ok(()),
        Found::Value(Value::Undefined) => Ok(()),
        Found::Value(v) if v.is_object() => Ok(()),
        Found::Value(_) => {
            Err("TypeError: object.constructor[Symbol.species] is not a constructor".into())
        }
    }
}

/// `value.name(...)` for an array method upstream calls (`value.map`, `node2.map`, ...): `Ok`
/// when it is the built-in `method`; otherwise V8's `TypeError: {expression}.{name} is not a
/// function` (`expression` is the source text upstream's dist has there).
pub fn array_method(
    value: &Value,
    name: &str,
    method: Method,
    expression: &str,
) -> Result<(), String> {
    match lookup(value, name)? {
        Found::Builtin(_, Intrinsic::Method(m)) if m == method => Ok(()),
        Found::Value(Value::Builtin(f)) if Method::of(f) == Some(method) => Ok(()),
        _ => Err(format!("TypeError: {expression}.{name} is not a function")),
    }
}

/// `value.constructor === ctor` for a built-in constructor (`validateType`'s check): the
/// `constructor` the chain finds is that built-in (a document's own `constructor` never is).
pub fn constructor_is(value: &Value, ctor: Ctor) -> bool {
    match lookup(value, "constructor") {
        Ok(Found::Builtin(_, Intrinsic::Ctor(c))) => c == ctor,
        // An own `constructor` holding the built-in constructor itself.
        Ok(Found::Value(Value::Builtin(f))) => f == ctor.builtin(),
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------
// Markdoc's checks (src/ast/base.ts, src/tag.ts)
// ---------------------------------------------------------------------------------------

/// `Ast.isAst(value)`: `!!value?.$$mdtype`, read through the chain.
pub fn is_ast(value: &Value) -> bool {
    !value.is_nullish() && is_truthy_at(value, "$$mdtype").unwrap_or(false)
}

/// `value?.$$mdtype === kind` (`Ast.isFunction`, `Ast.isVariable`, `Tag.isTag`), read
/// through the chain.
pub fn mdtype_is(value: &Value, kind: &str) -> bool {
    if value.is_nullish() {
        return false;
    }
    if let Value::Tag(_) = value {
        return kind == "Tag";
    }
    matches!(lookup(value, "$$mdtype"), Ok(Found::Value(Value::String(t))) if t == kind)
}

// ---------------------------------------------------------------------------------------
// JSON.stringify
// ---------------------------------------------------------------------------------------

/// `JSON.stringify(value)` (`gap` empty) or `JSON.stringify(value, null, gap)`: `Ok(None)`
/// where JS returns `undefined` (for `undefined` itself), `Err` (a `RangeError`) for arrays
/// and objects nested deeper than node's stack takes ([`super::MAX_JSON_DEPTH`] from the top of a
/// thread, less where the calls below charge the depth budget) and for a text past
/// [`MAX_STRING_LENGTH`] code units (as V8 finds it: while writing, in order). Lone surrogates
/// are written as `\udxxx` escapes, so the text is always well formed. An object's own
/// enumerable properties alone are written (a prototype, an array's other own properties
/// are not); a hole is `null`. A loop, at any depth.
pub fn json_stringify(value: &Value, gap: &str) -> Result<Option<String>, String> {
    let limit = usize::try_from(crate::depth::left() / super::JSON_COST).unwrap_or(usize::MAX);
    json_stringify_within(value, gap, limit)
}

/// A container [`json_stringify_within`] is writing.
enum JsonFrame {
    /// An array: its length, its present elements (index, value) and how far it has got.
    Array {
        length: u64,
        elements: std::vec::IntoIter<(u64, Value)>,
        /// The next index to write.
        at: u64,
        stepback: usize,
    },
    /// An object: its own enumerable properties, and whether one was written yet.
    Object {
        members: std::vec::IntoIter<(String, Value)>,
        any: bool,
        stepback: usize,
    },
}

/// The output of [`json_stringify_within`], measured in UTF-16 code units against V8's limit.
/// A text's UTF-16 length is at most its UTF-8 length, so nothing is counted until the text is
/// longer than the limit in bytes; past that, the code units are counted once, as it grows.
struct JsonOut {
    text: String,
    /// The code units of `text[..counted]`.
    units: u64,
    counted: usize,
}

impl JsonOut {
    /// The text's length in code units when it may be past the limit (`None`: it is not).
    fn units(&mut self) -> Option<u64> {
        if self.text.len() <= MAX_STRING_LENGTH {
            return None;
        }
        self.units += self.text[self.counted..].encode_utf16().count() as u64;
        self.counted = self.text.len();
        Some(self.units)
    }

    /// `Err` when the text is past V8's limit.
    fn check(&mut self) -> Result<(), String> {
        match self.units() {
            Some(units) if units > MAX_STRING_LENGTH as u64 => Err(INVALID_STRING_LENGTH.into()),
            _ => Ok(()),
        }
    }

    fn push(&mut self, s: &str) -> Result<(), String> {
        self.text.push_str(s);
        self.check()
    }

    /// `s` repeated `n` times (the holes of a sparse array), checked before it is written.
    fn repeat(&mut self, s: &str, n: u64) -> Result<(), String> {
        let bytes = (s.len() as u64).saturating_mul(n);
        if self.text.len() as u64 + bytes > MAX_STRING_LENGTH as u64 {
            let units = self.units().unwrap_or(self.text.len() as u64);
            let more = (s.encode_utf16().count() as u64).saturating_mul(n);
            if units + more > MAX_STRING_LENGTH as u64 {
                return Err(INVALID_STRING_LENGTH.into());
            }
        }
        for _ in 0..n {
            self.text.push_str(s);
        }
        self.check()
    }
}

/// What `SerializeJSONProperty` writes for a value held under `key`, before it is written:
/// the value itself, or what its `toJSON` (a function the value's chain finds: one of the
/// realm a document's object holds) returns, called with the key; a wrapper's primitive (a
/// wrapper a call made, and the realm's `String.prototype`, `Number.prototype` and
/// `Boolean.prototype`, which hold `""`, `0` and `false`). `None` for `undefined` and a
/// function: an object member left out, an array item `null`.
fn json_value<'a>(value: &'a Value, key: &str) -> Result<Option<Cow<'a, Value>>, String> {
    let to_json = match value {
        Value::Undefined => return Ok(None),
        Value::Null | Value::Boolean(_) | Value::Number(_) | Value::String(_) => {
            return Ok(Some(Cow::Borrowed(value)));
        }
        // Fast paths: `Array.prototype`, `Object.prototype` and `Tag.prototype` have no
        // `toJSON`, so only an own one counts.
        Value::Array(items) => items.named_get("toJSON").cloned(),
        Value::Object(o) if o.proto().is_object_prototype() => o.get("toJSON").cloned(),
        Value::Tag(_) => None,
        _ => match lookup(value, "toJSON")? {
            Found::Value(v) => Some(v),
            Found::Builtin(_, Intrinsic::Method(m)) => Some(Value::Builtin(m.builtin())),
            Found::Builtin(..) => None,
        },
    };
    let value = match to_json {
        Some(Value::Builtin(f)) if f.is_callable() => {
            // Called by `JSON.stringify` itself (see `realm::call::Site`).
            let called = crate::next::realm::call::at_site(crate::next::realm::Site::Json, || {
                crate::next::realm::call(f, value, &[Value::string(key)])
            })?;
            Cow::Owned(match called {
                Called::Value(v) => v,
                // A new wrapper object: written as its primitive.
                Called::Wrapper(primitive) => primitive,
            })
        }
        _ => Cow::Borrowed(value),
    };
    Ok(match &*value {
        Value::Undefined => None,
        Value::Builtin(b) => match b.data().wrapped {
            Some(Wrapped::String(s)) => Some(Cow::Owned(Value::string(s))),
            Some(Wrapped::Number(n)) => Some(Cow::Owned(Value::Number(n))),
            Some(Wrapped::Boolean(v)) => Some(Cow::Owned(Value::Boolean(v))),
            None if b.is_callable() => None,
            None => Some(value),
        },
        _ => Some(value),
    })
}

/// `SerializeJSONProperty` of a value (`undefined` written as `null`: the caller leaves an
/// object member that is `undefined` out before): a primitive written, or the frame of a
/// container to open.
fn serialize(
    value: &Value,
    out: &mut JsonOut,
    indent_len: usize,
) -> Result<Option<JsonFrame>, String> {
    // `toJSON`, looked up through the chain, is called when it is a function: no document
    // value and no built-in prototype on a document's chain has one.
    match value {
        Value::Undefined | Value::Null => out.push("null")?,
        Value::Boolean(b) => out.push(if *b { "true" } else { "false" })?,
        Value::Number(n) => {
            if n.is_finite() {
                out.text.push_str(&js::number_to_string(*n));
                out.check()?;
            } else {
                out.push("null")?;
            }
        }
        Value::String(s) => {
            quote_json_js_string(s, &mut out.text);
            out.check()?;
        }
        Value::Array(items) => {
            return Ok(Some(JsonFrame::Array {
                length: items.len() as u64,
                elements: items
                    .present()
                    .map(|(i, v)| (u64::from(i), v.clone()))
                    .collect::<Vec<_>>()
                    .into_iter(),
                at: 0,
                stepback: indent_len,
            }));
        }
        Value::Object(_) | Value::Tag(_) => {
            return Ok(Some(JsonFrame::Object {
                members: own_entries(value).into_iter(),
                any: false,
                stepback: indent_len,
            }));
        }
        // A built-in object (a function and a wrapper prototype are made something else
        // by `json_value`): `Array.prototype` an array of none, any other its own enumerable
        // members.
        Value::Builtin(b) if b.is_array() => {
            return Ok(Some(JsonFrame::Array {
                length: 0,
                elements: Vec::new().into_iter(),
                at: 0,
                stepback: indent_len,
            }));
        }
        Value::Builtin(_) => {
            return Ok(Some(JsonFrame::Object {
                members: own_entries(value).into_iter(),
                any: false,
                stepback: indent_len,
            }));
        }
    }
    Ok(None)
}

/// [`json_stringify`] with another nesting limit.
pub(crate) fn json_stringify_within(
    value: &Value,
    gap: &str,
    max_depth: usize,
) -> Result<Option<String>, String> {
    if value.is_undefined() {
        return Ok(None);
    }
    let mut out = JsonOut {
        text: String::new(),
        units: 0,
        counted: 0,
    };
    let mut indent = String::new();
    let mut stack: Vec<JsonFrame> = Vec::new();
    // Opens a container: `[` or `{`, unless it is an empty array (`[]`, closed at once).
    let open = |frame: JsonFrame,
                stack: &mut Vec<JsonFrame>,
                out: &mut JsonOut,
                indent: &mut String|
     -> Result<(), String> {
        if stack.len() >= max_depth {
            return Err(RANGE_ERROR.into());
        }
        match &frame {
            JsonFrame::Array { length: 0, .. } => return out.push("[]"),
            JsonFrame::Array { .. } => out.push("[")?,
            JsonFrame::Object { .. } => out.push("{")?,
        }
        indent.push_str(gap);
        stack.push(frame);
        Ok(())
    };
    let Some(value) = json_value(value, "")? else {
        return Ok(None);
    };
    if let Some(frame) = serialize(&value, &mut out, 0)? {
        open(frame, &mut stack, &mut out, &mut indent)?;
    }
    // `\n` and the indent before each member, with a gap.
    let newline = |out: &mut JsonOut, indent: &str| {
        out.text.push('\n');
        out.text.push_str(indent);
    };
    while let Some(top) = stack.last_mut() {
        match top {
            JsonFrame::Array {
                length,
                elements,
                at,
                stepback,
            } => {
                let next = elements.next();
                // The holes before the next element (or to the end): `null` each.
                let until = next.as_ref().map_or(*length, |(i, _)| *i);
                if *at < until {
                    let holes = until - *at;
                    let mut item = String::new();
                    if !gap.is_empty() {
                        item.push('\n');
                        item.push_str(&indent);
                    }
                    item.push_str("null");
                    if *at == 0 {
                        out.push(&item)?;
                        out.repeat(&format!(",{item}"), holes - 1)?;
                    } else {
                        out.repeat(&format!(",{item}"), holes)?;
                    }
                    *at = until;
                }
                let Some((i, element)) = next else {
                    let stepback = *stepback;
                    stack.pop();
                    if !gap.is_empty() {
                        newline(&mut out, &indent[..stepback]);
                    }
                    out.push("]")?;
                    indent.truncate(stepback);
                    continue;
                };
                // SerializeJSONArray: `,` between items, each on its own line with a gap.
                if i > 0 {
                    out.text.push(',');
                }
                if !gap.is_empty() {
                    newline(&mut out, &indent);
                }
                *at = i + 1;
                // An array item that is `undefined` (or a function) is `null`.
                match json_value(&element, &i.to_string())? {
                    None => out.push("null")?,
                    Some(element) => {
                        if let Some(frame) = serialize(&element, &mut out, indent.len())? {
                            open(frame, &mut stack, &mut out, &mut indent)?;
                        }
                    }
                }
            }
            JsonFrame::Object {
                members,
                any,
                stepback,
            } => {
                let Some((key, member)) = members.next() else {
                    let (any, stepback) = (*any, *stepback);
                    stack.pop();
                    if any && !gap.is_empty() {
                        newline(&mut out, &indent[..stepback]);
                    }
                    out.push("}")?;
                    indent.truncate(stepback);
                    continue;
                };
                // SerializeJSONObject: a member that is `undefined` is left out.
                let Some(member) = json_value(&member, &key)? else {
                    continue;
                };
                if *any {
                    out.text.push(',');
                }
                *any = true;
                if !gap.is_empty() {
                    newline(&mut out, &indent);
                }
                quote_json_string(&key, &mut out.text);
                out.text.push(':');
                if !gap.is_empty() {
                    out.text.push(' ');
                }
                if let Some(frame) = serialize(&member, &mut out, indent.len())? {
                    open(frame, &mut stack, &mut out, &mut indent)?;
                }
            }
        }
    }
    out.check()?;
    Ok(Some(out.text))
}

/// QuoteJSONString: `"` and `\` escaped, `\b \f \n \r \t` by name, other control characters
/// as `\u00xx`; everything else as it is.
pub fn quote_json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        quote_json_char(c, out);
    }
    out.push('"');
}

fn quote_json_char(c: char, out: &mut String) {
    match c {
        '"' => out.push_str("\\\""),
        '\\' => out.push_str("\\\\"),
        '\u{8}' => out.push_str("\\b"),
        '\u{c}' => out.push_str("\\f"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
        c => out.push(c),
    }
}

/// [`quote_json_string`] of a JS string: a lone surrogate is written as the escape
/// `\udxxx` (well-formed `JSON.stringify`, ES2019), a pair as the character it is.
pub fn quote_json_js_string(s: &JsString, out: &mut String) {
    let Some(text) = s.as_str() else {
        out.push('"');
        for c in char::decode_utf16(s.code_units()) {
            match c {
                Ok(c) => quote_json_char(c, out),
                Err(lone) => out.push_str(&format!("\\u{:04x}", lone.unpaired_surrogate())),
            }
        }
        out.push('"');
        return;
    };
    quote_json_string(text, out);
}
