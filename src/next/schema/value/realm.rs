//! The built-in prototypes at the end of every prototype chain: the object model's view of
//! the realm ([`crate::next::realm`], the realm agent's table of the built-in objects).
//!
//! Every prototype chain this crate walks ends in a built-in object ([`Builtin`], an index
//! into the realm's table; [`BuiltinProto`] is its name here) and asks the realm about it,
//! through three functions:
//!
//! - [`get_inherited`]`(proto, key, receiver)`: what a document reads (`$x.key`, `obj[key]`,
//!   `node.attributes[key]`) once the chain reaches `proto` (`super::get`,
//!   `super::object_get`): the realm's [`crate::next::realm::get_inherited`], the built-in's
//!   value (a function, a prototype, a constructor: [`super::Value::Builtin`]), an
//!   accessor's getter run with the receiver as `this` (`__proto__` answers the receiver's
//!   prototype, `Function.prototype.caller` throws), `undefined` where nothing has the key.
//! - [`has_inherited`]`(proto, key)`: whether the built-in chain has the name (`key in value`,
//!   `value[key] === undefined`).
//! - [`intrinsic`]`(proto, key)`: what an internal operation finds there, by identity: one of
//!   the built-in methods and constructors the operations of [`super`] carry out themselves
//!   over the object model ([`Method`], [`Ctor`]: `ToPrimitive`'s `toString`/`valueOf`,
//!   `Array.prototype.join`, `map`, `reduce`, upstream's AST classes' `resolve`, the
//!   constructors `validateType` compares), the `__proto__` accessor, or another built-in
//!   value ([`Intrinsic::Other`]), which an operation reads with [`get_inherited`] and, when
//!   it calls it, calls with [`crate::next::realm::call`].
//!
//! A call whose `this` is itself a built-in object (`String(Array.prototype)`), and every
//! function that is not one of the [`Method`]s, is the realm's [`crate::next::realm::call`].

use super::Value;
pub use crate::next::realm::Builtin;
use crate::next::realm::{Data, Getter, Slot};

/// A built-in prototype object (any object of the realm's table can be one: a document can
/// set a prototype to `$x.__proto__` or `$f.constructor`).
pub type BuiltinProto = Builtin;

/// A built-in method an operation here calls, found by name on a built-in chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    ObjectToString,
    ObjectValueOf,
    ArrayToString,
    ArrayJoin,
    ArrayMap,
    ArrayFlatMap,
    ArrayReduce,
    ArrayIncludes,
    StringToString,
    StringValueOf,
    StringIncludes,
    NumberToString,
    NumberValueOf,
    BooleanToString,
    BooleanValueOf,
    /// `Variable.prototype.resolve`.
    VariableResolve,
    /// `Function.prototype.resolve` (Markdoc's AST class).
    AstFunctionResolve,
    /// `Node.prototype.resolve`.
    NodeResolve,
}

/// A built-in constructor (what `constructor` holds on a built-in prototype), compared by
/// identity (`value.constructor === type`, `ArraySpeciesCreate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ctor {
    Object,
    Array,
    String,
    Number,
    Boolean,
    Tag,
    Variable,
    AstFunction,
    Node,
}

/// What an internal operation finds for a name on a built-in chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intrinsic {
    /// No built-in on the chain has the name: `undefined`.
    Absent,
    /// `Object.prototype.__proto__`: an accessor (its getter returns the receiver's prototype,
    /// its setter replaces it with an object or `null`).
    ProtoAccessor,
    Method(Method),
    Ctor(Ctor),
    /// Another built-in value (a function or an object; truthy, callable or not, but nothing
    /// an operation here calls).
    Other,
}

/// `Object.getOwnPropertyNames(Object.prototype)` (node 22.22.1).
pub const OBJECT_PROTOTYPE: [&str; 12] = [
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "__proto__",
    "toLocaleString",
];

impl Method {
    /// The realm's function this is.
    pub fn builtin(self) -> Builtin {
        match self {
            Method::ObjectToString => Builtin::OBJECT_PROTOTYPE_TO_STRING,
            Method::ObjectValueOf => Builtin::OBJECT_PROTOTYPE_VALUE_OF,
            Method::ArrayToString => Builtin::ARRAY_PROTOTYPE_TO_STRING,
            Method::ArrayJoin => Builtin::ARRAY_PROTOTYPE_JOIN,
            Method::ArrayMap => Builtin::ARRAY_PROTOTYPE_MAP,
            Method::ArrayFlatMap => Builtin::ARRAY_PROTOTYPE_FLAT_MAP,
            Method::ArrayReduce => Builtin::ARRAY_PROTOTYPE_REDUCE,
            Method::ArrayIncludes => Builtin::ARRAY_PROTOTYPE_INCLUDES,
            Method::StringToString => Builtin::STRING_PROTOTYPE_TO_STRING,
            Method::StringValueOf => Builtin::STRING_PROTOTYPE_VALUE_OF,
            Method::StringIncludes => Builtin::STRING_PROTOTYPE_INCLUDES,
            Method::NumberToString => Builtin::NUMBER_PROTOTYPE_TO_STRING,
            Method::NumberValueOf => Builtin::NUMBER_PROTOTYPE_VALUE_OF,
            Method::BooleanToString => Builtin::BOOLEAN_PROTOTYPE_TO_STRING,
            Method::BooleanValueOf => Builtin::BOOLEAN_PROTOTYPE_VALUE_OF,
            Method::VariableResolve => Builtin::VARIABLE_PROTOTYPE_RESOLVE,
            Method::AstFunctionResolve => Builtin::AST_FUNCTION_PROTOTYPE_RESOLVE,
            Method::NodeResolve => Builtin::NODE_PROTOTYPE_RESOLVE,
        }
    }

    /// The method a built-in function is, if it is one of these.
    pub fn of(f: Builtin) -> Option<Method> {
        const ALL: [Method; 18] = [
            Method::ObjectToString,
            Method::ObjectValueOf,
            Method::ArrayToString,
            Method::ArrayJoin,
            Method::ArrayMap,
            Method::ArrayFlatMap,
            Method::ArrayReduce,
            Method::ArrayIncludes,
            Method::StringToString,
            Method::StringValueOf,
            Method::StringIncludes,
            Method::NumberToString,
            Method::NumberValueOf,
            Method::BooleanToString,
            Method::BooleanValueOf,
            Method::VariableResolve,
            Method::AstFunctionResolve,
            Method::NodeResolve,
        ];
        ALL.into_iter().find(|m| m.builtin() == f)
    }
}

impl Ctor {
    /// The realm's constructor this is.
    pub fn builtin(self) -> Builtin {
        match self {
            Ctor::Object => Builtin::OBJECT,
            Ctor::Array => Builtin::ARRAY,
            Ctor::String => Builtin::STRING,
            Ctor::Number => Builtin::NUMBER,
            Ctor::Boolean => Builtin::BOOLEAN,
            Ctor::Tag => Builtin::TAG,
            Ctor::Variable => Builtin::VARIABLE,
            Ctor::AstFunction => Builtin::AST_FUNCTION,
            Ctor::Node => Builtin::NODE,
        }
    }

    /// The constructor a built-in object is, if it is one of these.
    pub fn of(f: Builtin) -> Option<Ctor> {
        const ALL: [Ctor; 9] = [
            Ctor::Object,
            Ctor::Array,
            Ctor::String,
            Ctor::Number,
            Ctor::Boolean,
            Ctor::Tag,
            Ctor::Variable,
            Ctor::AstFunction,
            Ctor::Node,
        ];
        ALL.into_iter().find(|c| c.builtin() == f)
    }
}

/// `key in proto`, for a built-in object (and so for whatever inherits from it).
pub fn has_inherited(proto: BuiltinProto, key: &str) -> bool {
    proto.has_property(key)
}

/// `receiver[key]` once the chain has reached `proto` (a document's read: `$x.key`,
/// `obj[key]`): the realm's answer ([`crate::next::realm::get_inherited`]). The receiver is
/// the value the read started from, which an accessor's getter gets as `this` (`__proto__`
/// returns its prototype).
pub fn get_inherited(proto: BuiltinProto, key: &str, receiver: &Value) -> Result<Value, String> {
    crate::next::realm::get_inherited(proto, key, receiver)
}

/// What an internal operation finds for `key` on `proto`'s chain (`proto` itself included),
/// without running a getter.
pub fn intrinsic(proto: BuiltinProto, key: &str) -> Intrinsic {
    let mut object = Some(proto);
    while let Some(o) = object {
        if let Some(prop) = o.own(key) {
            return match &prop.slot {
                Slot::Accessor {
                    get: Some(Getter::Proto),
                    ..
                } => Intrinsic::ProtoAccessor,
                Slot::Data(Data::Object(b)) => Method::of(*b)
                    .map(Intrinsic::Method)
                    .or_else(|| Ctor::of(*b).map(Intrinsic::Ctor))
                    .unwrap_or(Intrinsic::Other),
                _ => Intrinsic::Other,
            };
        }
        object = o.proto();
    }
    Intrinsic::Absent
}
