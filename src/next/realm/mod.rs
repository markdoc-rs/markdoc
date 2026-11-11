//! The JavaScript realm upstream's values live in: the built-in objects a value reaches by
//! reading properties, and what calling one of them does.
//!
//! Owner: the realm agent (A1 of the closing round).
//!
//! Upstream reads a property with JavaScript's `[[Get]]`, which walks the value's prototype
//! chain, so a document reaches everything a value inherits: `$s.trim` (a function),
//! `$list.map`, `$x.constructor` (`Object`, `Number`, ...), `$x.constructor.name`,
//! `$x.__proto__` (a prototype object), `$n.toFixed.length`, `$t.constructor` (Markdoc's `Tag`
//! class, whose `String()` is its source text in upstream's dist), and getters such as
//! `Function.prototype.caller`, which throws. Every such object is in this module's table
//! (`table.rs`, generated from node 22.22.1 and the 0.5.11 dist by `tools/gen_realm.cjs`,
//! whose `--check` re-checks it against node): the constructors and prototypes of every kind
//! of value a variable, an attribute or a function's result can hold (primitives, plain
//! objects, arrays, functions, `Tag`; through the harness's JSON config conventions the
//! constructors, Markdoc's classes `Class`, `Id`, `ConditionalAttributeType` and
//! `PartialFile`, and `RegExp`; upstream's AST classes `Node`, `Variable` and `Function`,
//! whose instances the object model makes values), and to a fixpoint every object their
//! string-keyed properties hold, every getter that answers one object (`Iterator.prototype.
//! constructor`) and every prototype (214 objects; `Node.prototype.walk`, a generator method,
//! brings in `GeneratorFunction`, `Generator.prototype` and `Iterator` with its helpers). For
//! each object the table has its prototype, its own string-keyed properties in
//! `[[OwnPropertyKeys]]` order with their attributes (a data property's value, or an
//! accessor's getter: [`Getter`]), what `Object.prototype.toString` calls its class, the
//! primitive a wrapper prototype holds, for a function the text `Function.prototype.toString`
//! gives, and flags ([`CALLABLE`], [`CONSTRUCTOR`], [`CLASS`], [`IS_ARRAY`], [`FRAMELESS`]).
//!
//! A value that is one of these objects is [`Value::Builtin`]: a [`Builtin`], an index into
//! the table. Built-in objects are the realm's own, one each, and nothing a document or a
//! config does changes them, so the index is the object's identity (`===` and SameValueZero
//! compare indices) and the table is all there is to it. The operations upstream applies to
//! values handle it as JavaScript does (`schema::value`): `typeof` is `'function'` for a
//! callable one and `'object'` otherwise, every one is truthy (to JS and to Markdoc's
//! `truthy`), `String()` calls the `toString` its chain finds (`String(Array.prototype)` is
//! `""`, `String(Number.prototype)` `"0"`, a function its source text), `JSON.stringify`
//! writes a function as nothing (`null` in an array) and a wrapper prototype as its primitive,
//! `Array.isArray(Array.prototype)` is true (`flatMap` spreads it as no items),
//! `Object.entries` sees the enumerable own properties (only `Tag.isTag` is one, so
//! `class=$t.constructor` is `"isTag"`), `Tag.isTag` is false, the HTML renderer renders
//! nothing for one and `String()` of it in an attribute, a validator's type check compares
//! `value.constructor` through the chain.
//!
//! # The contract with the object model
//!
//! Every property read that reaches a built-in prototype goes through one function, and the
//! object model's chain walk (A2: an object whose prototype a document set) calls it where its
//! own chain ends at a built-in:
//!
//! ```text
//! pub fn get_inherited(proto: Builtin, key: &str, receiver: &Value) -> Result<Value, String>
//! ```
//!
//! `[[Get]]` continued at the built-in object `proto`: `proto`'s own property `key` if it has
//! one, else its prototype's, and so on up the table's chain; `Ok(Value::Undefined)` when no
//! object on the chain has `key`. A data property gives its value (a primitive, or another
//! [`Value::Builtin`]); an accessor runs its getter with `receiver` (the value the read
//! started on, not `proto`) as `this`: `__proto__` answers [`prototype_of`]`(receiver)`,
//! `Function.prototype.caller`/`arguments` throw V8's `TypeError` ([`RESTRICTED`]),
//! `RegExp.prototype`'s flag, `source` and `flags` getters answer for `RegExp.prototype` and
//! throw for any other object, `Iterator.prototype.constructor` answers `Iterator`. `Err` is
//! what JS throws, as `Name: message`. Keys are property keys as strings (`ToPropertyKey`
//! already applied: `"0"`, `"length"`). The default lookups go through it:
//! `schema::value::get_property` of a primitive, an array, a plain object, a tag or a built-in
//! continues at `String.prototype`, `Number.prototype`, `Boolean.prototype`,
//! `Array.prototype`, `Object.prototype`, `Tag.prototype` or the built-in's own prototype
//! once the value's own properties do not have the key.
//!
//! [`intrinsic`]`(proto, key) -> Option<Builtin>` names the built-in a chain holds under a
//! key without running a getter (what an internal operation would call: `Array.prototype.
//! toString` for `String(array)`).
//!
//! [`prototype_of`]`(value) -> Value` is `Object.getPrototypeOf(ToObject(value))` for the
//! default prototypes (what the `__proto__` getter answers): the object model answers for an
//! object whose prototype a document set and calls this for every other value.
//! [`inherited_by_plain_object`]`(key)` is `get_inherited(Builtin::OBJECT_PROTOTYPE, key,
//! plain object)`, which the config lookups use (`config.tags[name]` of a name every object
//! inherits, `node.attributes[key]`).
//!
//! # Calls
//!
//! Upstream calls a built-in function only where it converts a value: `String(value)`, a
//! template literal and a property key call the `toString` (then the `valueOf`) the value's
//! chain finds, with no argument; `JSON.stringify` calls a `toJSON` with the member's key; a
//! `matches` that is a function is called with the config (`{"$type": name}` in a JSON
//! config); and the built-ins call each other (`join` converts the items, `toLocaleString`
//! invokes the items'). For a built-in object these are its own prototypes' methods with the
//! object itself as `this`; an object a document makes can hold any built-in function under
//! those keys (`{toString: $s.trim}`), which upstream then calls with that object as `this`.
//! [`call`] is that call, for every function in the table, as V8 runs it (see its module).
//!
//! # Divergences from upstream
//!
//! - What a call cannot reproduce (a function body or a locale V8 compiles or looks up with
//!   data this crate does not have, a call that sets properties of a document's object, ...):
//!   see [`call`]'s module documentation.
//! - `RegExp`'s legacy static properties (`RegExp.$1`, `RegExp.lastMatch`, ...): V8 keeps the
//!   last successful match of any regular expression the process ran (markdown-it's own
//!   included), so their values depend on what node did before; here they read as in a realm
//!   in which nothing has matched yet (`""`) ([`Getter::RegExpLegacy`]). Only a value that is
//!   `RegExp` itself reaches them, and no value here is (the harness's `{"$regex": ...}` as a
//!   value is not modeled).

pub(crate) mod call;
mod call_array;
mod call_ast;
mod call_string;
mod collation;
// Generated by tools/gen_realm.cjs, whose `--check` compares it byte for byte.
#[rustfmt::skip]
mod table;
#[cfg(test)]
mod tests;

pub use call::{Called, MAX_CALL_DEPTH, Site, call};
pub use table::{MARKDOC_VERSION, NODE_VERSION};

use crate::next::schema::value::Value;
use std::fmt;

/// A built-in object of the realm: an index into the table. Its identity: two `Builtin`s are
/// the same object exactly when they are equal. The well-known ones are associated constants
/// (`Builtin::OBJECT_PROTOTYPE`, `Builtin::STRING_PROTOTYPE_TRIM`, `Builtin::TAG`, ...), one
/// for every object of the table, named after the first path that reaches it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Builtin(u16);

/// One object of the table.
#[derive(Debug)]
pub struct ObjectData {
    /// The first path that reaches it (`String.prototype.trim`).
    pub name: &'static str,
    /// [`CALLABLE`], [`CONSTRUCTOR`], [`CLASS`], [`IS_ARRAY`].
    pub flags: u8,
    /// `Object.getPrototypeOf(o)`.
    pub proto: Option<Builtin>,
    /// What `Object.prototype.toString` calls it (`Object`, `Function`, `Array`, `String`,
    /// ...): `[object ${class}]`.
    pub class: &'static str,
    /// The primitive a wrapper prototype holds: `String.prototype`'s `[[StringData]]` (`""`),
    /// `Number.prototype`'s `[[NumberData]]` (0), `Boolean.prototype`'s `[[BooleanData]]`
    /// (`false`).
    pub wrapped: Option<Wrapped>,
    /// `Function.prototype.toString` of a function.
    pub source: Option<&'static str>,
    /// The own string-keyed properties, in `[[OwnPropertyKeys]]` order.
    pub props: &'static [Prop],
}

/// The object is callable (`typeof` is `'function'`).
pub const CALLABLE: u8 = 1;
/// The object is a constructor (`new` accepts it).
pub const CONSTRUCTOR: u8 = 2;
/// The object is a class (calling it without `new` throws).
pub const CLASS: u8 = 4;
/// `Array.isArray` is true (`Array.prototype` is an array).
pub const IS_ARRAY: u8 = 8;
/// V8 runs the function without a frame stack walks see (a C++ or assembly builtin): what it
/// calls is called from the site that called it (see [`Site`]).
pub const FRAMELESS: u8 = 16;

/// A property's `[[Writable]]`.
pub const WRITABLE: u8 = 1;
/// A property's `[[Enumerable]]`.
pub const ENUMERABLE: u8 = 2;
/// A property's `[[Configurable]]`.
pub const CONFIGURABLE: u8 = 4;

/// An own property.
#[derive(Debug)]
pub struct Prop {
    pub key: &'static str,
    pub slot: Slot,
    /// [`WRITABLE`], [`ENUMERABLE`], [`CONFIGURABLE`].
    pub attrs: u8,
}

/// A data property's value or an accessor's functions.
#[derive(Debug)]
pub enum Slot {
    Data(Data),
    /// `get` is what the getter does (`None`: no getter, the property reads as `undefined`);
    /// `set` whether there is a setter.
    Accessor {
        get: Option<Getter>,
        set: bool,
    },
}

/// A value the table holds.
#[derive(Debug, Clone, Copy)]
pub enum Data {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(&'static str),
    Object(Builtin),
}

/// What a getter of the table does with the value the read started on (`this`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Getter {
    /// `Object.prototype.__proto__`: the receiver's prototype ([`prototype_of`]).
    Proto,
    /// `Function.prototype`'s `caller` and `arguments`: throw V8's `TypeError` for every
    /// receiver (every function of the table is strict or built in).
    Restricted,
    /// `RegExp.prototype`'s flag getters (`hasIndices` `d`, `global` `g`, `ignoreCase` `i`,
    /// `multiline` `m`, `dotAll` `s`, `unicode` `u`, `unicodeSets` `v`, `sticky` `y`):
    /// `undefined` for `RegExp.prototype` itself, a `TypeError` for any other object that is
    /// not a RegExp.
    RegExpFlag(u8),
    /// `RegExp.prototype.source`: `"(?:)"` for `RegExp.prototype`.
    RegExpSource,
    /// `RegExp.prototype.flags`: each flag getter read in turn.
    RegExpFlags,
    /// `RegExp`'s legacy static properties (`RegExp.$1`, `RegExp.input`, `RegExp.lastMatch`,
    /// ...). V8 keeps them per realm: the last successful match of any regular expression the
    /// process ran (markdown-it's own included). No value here holds a RegExp, so a document
    /// cannot reach them; read anyway (a Rust caller's [`Builtin::REGEXP`]), they are what a
    /// realm in which nothing has matched yet gives: `""`.
    RegExpLegacy(Legacy),
    /// A getter that returns this object whatever `this` is (`Iterator.prototype.constructor`
    /// returns `Iterator`).
    Constant(Builtin),
}

/// Which legacy static property of `RegExp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Legacy {
    Input,
    LastMatch,
    LastParen,
    LeftContext,
    RightContext,
    Paren(u8),
}

/// The primitive a wrapper prototype holds.
#[derive(Debug, Clone, Copy)]
pub enum Wrapped {
    String(&'static str),
    Number(f64),
    Boolean(bool),
}

/// V8's error for reading `caller` or `arguments` of a strict or built-in function.
pub const RESTRICTED: &str = "TypeError: 'caller', 'callee', and 'arguments' properties may not be accessed on strict mode functions or the arguments objects for calls to them";

impl Builtin {
    /// Every object of the table, in its order.
    pub fn all() -> impl Iterator<Item = Builtin> {
        (0..table::OBJECTS.len()).map(|i| Builtin(i as u16))
    }

    /// The table's entry.
    pub fn data(self) -> &'static ObjectData {
        &table::OBJECTS[self.0 as usize]
    }

    /// The first path that reaches the object (`String.prototype.trim`).
    pub fn name(self) -> &'static str {
        self.data().name
    }

    /// `typeof o === 'function'`.
    pub fn is_callable(self) -> bool {
        self.data().flags & CALLABLE != 0
    }

    /// `new` accepts it.
    pub fn is_constructor(self) -> bool {
        self.data().flags & CONSTRUCTOR != 0
    }

    /// A class (calling it without `new` throws).
    pub fn is_class(self) -> bool {
        self.data().flags & CLASS != 0
    }

    /// `Array.isArray(o)`.
    pub fn is_array(self) -> bool {
        self.data().flags & IS_ARRAY != 0
    }

    /// V8 runs the function without a frame of its own (see [`FRAMELESS`]).
    pub fn is_frameless(self) -> bool {
        self.data().flags & FRAMELESS != 0
    }

    /// `Object.getPrototypeOf(o)`.
    pub fn proto(self) -> Option<Builtin> {
        self.data().proto
    }

    /// `Function.prototype.toString` of a function.
    pub fn source(self) -> Option<&'static str> {
        self.data().source
    }

    /// The own property `key`.
    pub fn own(self, key: &str) -> Option<&'static Prop> {
        self.data().props.iter().find(|p| p.key == key)
    }

    /// The own string keys, in `[[OwnPropertyKeys]]` order.
    pub fn own_keys(self) -> impl Iterator<Item = &'static str> {
        self.data().props.iter().map(|p| p.key)
    }

    /// `o[key]` with `receiver` as the getters' `this`: the own property, else the chain's
    /// ([`get_inherited`]).
    pub fn get(self, key: &str, receiver: &Value) -> Result<Value, String> {
        get_inherited(self, key, receiver)
    }

    /// `key in o`: an own or inherited property.
    pub fn has_property(self, key: &str) -> bool {
        let mut object = Some(self);
        while let Some(o) = object {
            if o.own(key).is_some() {
                return true;
            }
            object = o.proto();
        }
        false
    }

    /// Is `ancestor` on the object's prototype chain (the object itself not counted)?
    pub fn inherits_from(self, ancestor: Builtin) -> bool {
        let mut object = self.proto();
        while let Some(o) = object {
            if o == ancestor {
                return true;
            }
            object = o.proto();
        }
        false
    }
}

impl fmt::Debug for Builtin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl Data {
    /// The value.
    pub fn to_value(self) -> Value {
        match self {
            Data::Undefined => Value::Undefined,
            Data::Null => Value::Null,
            Data::Boolean(b) => Value::Boolean(b),
            Data::Number(n) => Value::Number(n),
            Data::String(s) => Value::string(s),
            Data::Object(o) => Value::Builtin(o),
        }
    }
}

/// `[[Get]]` continued at the built-in prototype `proto`: the value of `key` on `proto` or on
/// its prototype chain, `undefined` when none has it, with `receiver` (the value the read
/// started on) as the getters' `this`. `Err` where a getter throws (`Function.prototype.caller`).
///
/// This is the function the object model's chain walk calls when it reaches a built-in
/// prototype, and the one every default lookup goes through (see the module's
/// documentation).
pub fn get_inherited(proto: Builtin, key: &str, receiver: &Value) -> Result<Value, String> {
    let mut object = Some(proto);
    while let Some(o) = object {
        if let Some(prop) = o.own(key) {
            return match &prop.slot {
                Slot::Data(data) => Ok(data.to_value()),
                Slot::Accessor { get: None, .. } => Ok(Value::Undefined),
                Slot::Accessor {
                    get: Some(getter), ..
                } => run_getter(*getter, receiver),
            };
        }
        object = o.proto();
    }
    Ok(Value::Undefined)
}

/// The built-in object `proto`'s chain holds under `key` as a data property, without running
/// a getter: what the object model's own operations use to name the built-in method a value
/// would call (`intrinsic(Builtin::ARRAY_PROTOTYPE, "toString")` is
/// `Some(Builtin::ARRAY_PROTOTYPE_TO_STRING)`), so that it can tell a value whose method is
/// still the built-in from one whose own property or prototype replaced it. `None` when the
/// chain has no such data property or it holds a primitive.
pub fn intrinsic(proto: Builtin, key: &str) -> Option<Builtin> {
    let mut object = Some(proto);
    while let Some(o) = object {
        if let Some(prop) = o.own(key) {
            return match prop.slot {
                Slot::Data(Data::Object(b)) => Some(b),
                _ => None,
            };
        }
        object = o.proto();
    }
    None
}

/// The names of `RegExp.prototype`'s flag getters, by flag.
fn flag_getter_name(flag: u8) -> &'static str {
    match flag {
        b'd' => "hasIndices",
        b'g' => "global",
        b'i' => "ignoreCase",
        b'm' => "multiline",
        b's' => "dotAll",
        b'u' => "unicode",
        b'v' => "unicodeSets",
        _ => "sticky",
    }
}

/// `RegExp.prototype.flags`' order.
const FLAGS: [u8; 8] = *b"dgimsuvy";

/// A getter of the table, called with `receiver` as `this`.
fn run_getter(getter: Getter, receiver: &Value) -> Result<Value, String> {
    match getter {
        Getter::Proto => Ok(prototype_of(receiver)),
        Getter::Restricted => Err(RESTRICTED.into()),
        Getter::RegExpFlag(flag) => match receiver {
            Value::Builtin(Builtin::REGEXP_PROTOTYPE) => Ok(Value::Undefined),
            _ if is_object(receiver) => Err(format!(
                "TypeError: RegExp.prototype.{} getter called on non-RegExp object",
                flag_getter_name(flag)
            )),
            _ => Err(format!(
                "TypeError: RegExp.prototype.{} getter called on non-object {}",
                flag_getter_name(flag),
                call::describe_primitive(receiver)
            )),
        },
        Getter::RegExpSource => match receiver {
            Value::Builtin(Builtin::REGEXP_PROTOTYPE) => Ok(Value::string("(?:)")),
            _ if is_object(receiver) => {
                Err("TypeError: RegExp.prototype.source getter called on non-RegExp object".into())
            }
            _ => Err(format!(
                "TypeError: RegExp.prototype.source getter called on non-object {}",
                call::describe_primitive(receiver)
            )),
        },
        Getter::RegExpFlags => {
            if !is_object(receiver) {
                return Err(format!(
                    "TypeError: RegExp.prototype.flags getter called on non-object {}",
                    call::describe_primitive(receiver)
                ));
            }
            let mut out = String::new();
            for flag in FLAGS {
                let value =
                    crate::next::schema::value::get_property(receiver, flag_getter_name(flag))?;
                if value.to_boolean() {
                    out.push(flag as char);
                }
            }
            Ok(Value::string(out))
        }
        Getter::RegExpLegacy(_) => Ok(Value::string("")),
        Getter::Constant(object) => Ok(Value::Builtin(object)),
    }
}

/// `typeof value` is `'object'` (not `null`) or `'function'`.
pub fn is_object(value: &Value) -> bool {
    matches!(
        value,
        Value::Array(_) | Value::Object(_) | Value::Tag(_) | Value::Builtin(_)
    )
}

/// `Object.getPrototypeOf(ToObject(value))`: what the `__proto__` getter returns for a value
/// read through it. Every value here has its default prototype: a boolean's, number's or
/// string's is its wrapper's (`Boolean.prototype`, ...), an array's `Array.prototype`, a plain
/// object's `Object.prototype`, a tag's `Tag.prototype`, a built-in object's the table's
/// (`null` for `Object.prototype`). `undefined` and `null` have none (`null` here; reading a
/// property of either throws before a getter runs).
///
/// The object model answers for an object whose prototype a document set (see the module's
/// documentation).
pub fn prototype_of(value: &Value) -> Value {
    let proto = match value {
        Value::Undefined | Value::Null => return Value::Null,
        Value::Boolean(_) => Builtin::BOOLEAN_PROTOTYPE,
        Value::Number(_) => Builtin::NUMBER_PROTOTYPE,
        Value::String(_) => Builtin::STRING_PROTOTYPE,
        Value::Array(_) => Builtin::ARRAY_PROTOTYPE,
        // The object model's prototype: a built-in, `null`, or a document's object.
        Value::Object(o) => match o.proto() {
            crate::next::schema::value::Proto::Builtin(b) => *b,
            crate::next::schema::value::Proto::Null => return Value::Null,
            crate::next::schema::value::Proto::Value(v) => return v.clone(),
        },
        Value::Tag(_) => Builtin::TAG_PROTOTYPE,
        Value::Builtin(b) => match b.proto() {
            Some(p) => p,
            None => return Value::Null,
        },
    };
    Value::Builtin(proto)
}

/// What a plain object whose prototype is `Object.prototype` inherits under `key`: the value
/// of `Object.prototype`'s property (for `__proto__`, its getter's answer: `Object.prototype`
/// itself), `None` when `Object.prototype` has no such property. No getter of
/// `Object.prototype` throws, so this is `[[Get]]` of every such object for a key it does not
/// have itself.
pub fn inherited_by_plain_object(key: &str) -> Option<Value> {
    let prop = Builtin::OBJECT_PROTOTYPE.own(key)?;
    Some(match &prop.slot {
        Slot::Data(data) => data.to_value(),
        Slot::Accessor {
            get: Some(Getter::Proto),
            ..
        } => Value::Builtin(Builtin::OBJECT_PROTOTYPE),
        Slot::Accessor { .. } => unreachable!("Object.prototype's only accessor is __proto__"),
    })
}

/// The value the harness's config convention `{"$type": name}` stands for
/// (`tests/next/oracle.cjs`'s `TYPES[name]`, a plain object holding the constructors
/// `String`, `Number`, `Boolean`, `Object`, `Array` and Markdoc's classes `Class`, `Id`,
/// `ConditionalAttributeType`, `PartialFile`): one of those, a name every plain object
/// inherits (`TYPES.toString` is `Object.prototype.toString`, `TYPES.__proto__`
/// `Object.prototype`), or `undefined`.
pub fn harness_type(name: &str) -> Value {
    let own = match name {
        "String" => Builtin::STRING,
        "Number" => Builtin::NUMBER,
        "Boolean" => Builtin::BOOLEAN,
        "Object" => Builtin::OBJECT,
        "Array" => Builtin::ARRAY,
        "Class" => Builtin::CLASS,
        "Id" => Builtin::ID,
        "ConditionalAttributeType" => Builtin::CONDITIONAL_ATTRIBUTE_TYPE,
        "PartialFile" => Builtin::PARTIAL_FILE,
        _ => {
            return get_inherited(Builtin::OBJECT_PROTOTYPE, name, &Value::empty_object())
                .unwrap_or(Value::Undefined);
        }
    };
    Value::Builtin(own)
}
