//! The JavaScript objects upstream's parser holds while it parses: attribute objects, the
//! values the tag grammar made and a hand-made token's `meta`, with their identity, so that
//! what `annotate` does to one is seen through every reference to it.
//!
//! Owner: the model agent (A2).
//!
//! Upstream's `annotate` pushes each attribute object onto `node.annotations` and assigns its
//! value to `node.attributes`: the annotation and the attribute hold the same object, and a
//! later `.name` (`node.attributes.class[name] = value`) changes it for both. An attribute
//! named `__proto__` makes its value the attributes' prototype (`Object.prototype`'s setter),
//! and later reads (`node.attributes[name] !== undefined`, `node.attributes.class`) go
//! through it: `.name` can change an object the prototype holds, which the `__proto__`
//! annotation shows. Here every object is in a [`Heap`] and a value refers to it by [`Id`];
//! once the tokens are parsed (nothing changes them after that), [`Heap::snapshot`] makes the
//! AST's values: the plain [`Scalar`] variants where the object is plain data, else
//! [`Scalar::Js`] holding the [`Value`] the object is (its prototype, holes, extra
//! properties), objects shared between references shared there too.
//!
//! The operations are JavaScript's, for the objects a parse can make: `[[Get]]` through the
//! prototype chain (ending in a built-in prototype, which the realm answers for: see
//! [`crate::next::schema::value::realm`]), `[[Set]]` (`OrdinarySet`, an array's element and
//! `length`, the `__proto__` setter, a primitive's `TypeError`s in strict mode), and
//! `JSON.parse` with `Ast.fromJSON`'s reviver for a hand-made token's `meta`.

use crate::grammar;
use crate::markdown_it::js;
use crate::next::schema::value::{
    self, Array, BuiltinProto, Intrinsic, Object, Proto, Value, realm,
};
use crate::types::{JsObject, Scalar, array_index};
use indexmap::IndexMap;
use std::collections::{BTreeMap, HashMap};

/// An object in a [`Heap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Id(u32);

/// A value while the tokens are parsed: a primitive, or an object in the [`Heap`].
#[derive(Debug, Clone)]
pub(super) enum HValue {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Object(Id),
}

/// What made an object, which gives it its built-in prototype (unless a document replaced
/// it) and its own properties' kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Class {
    /// `{}`: `Object.prototype`.
    Object,
    /// `[]`: `Array.prototype`, elements and `length`.
    Array,
    /// `new Variable(...)`: `Variable.prototype`.
    Variable,
    /// `new Function(...)` (upstream's `Ast.Function`): `Function.prototype`.
    Function,
    /// `new Node(...)` (a hand-made token's `meta`, revived by `Ast.fromJSON`):
    /// `Node.prototype`.
    Node,
}

impl Class {
    fn builtin(self) -> BuiltinProto {
        match self {
            Class::Object => BuiltinProto::OBJECT_PROTOTYPE,
            Class::Array => BuiltinProto::ARRAY_PROTOTYPE,
            Class::Variable => BuiltinProto::VARIABLE_PROTOTYPE,
            Class::Function => BuiltinProto::AST_FUNCTION_PROTOTYPE,
            Class::Node => BuiltinProto::NODE_PROTOTYPE,
        }
    }
}

/// An object's prototype.
#[derive(Debug, Clone)]
pub(super) enum HProto {
    /// Its class's built-in prototype.
    Default,
    Null,
    Object(Id),
}

#[derive(Debug, Clone)]
struct HObject {
    class: Class,
    /// Own properties, in a JS object's order (an array's elements apart).
    props: JsObject<HValue>,
    /// An array's elements (holes absent) and its `length`.
    elements: BTreeMap<u32, HValue>,
    length: u32,
    proto: HProto,
}

/// What `[[Get]]` finds along a chain.
pub(super) enum HFound {
    Value(HValue),
    /// A built-in prototype has the key (or, [`Intrinsic::Absent`], none on the chain has).
    Builtin(Intrinsic),
}

impl HFound {
    pub(super) fn is_undefined(&self) -> bool {
        matches!(
            self,
            HFound::Value(HValue::Undefined) | HFound::Builtin(Intrinsic::Absent)
        )
    }
}

/// JS `ToBoolean` of a primitive or an object.
pub(super) fn truthy(value: &HValue) -> bool {
    match value {
        HValue::Undefined | HValue::Null => false,
        HValue::Bool(b) => *b,
        HValue::Number(n) => !(*n == 0.0 || n.is_nan()),
        HValue::String(s) => !s.is_empty(),
        HValue::Object(_) => true,
    }
}

/// `RangeError: Maximum call stack size exceeded`, past [`super::MAX_VALUE_DEPTH`].
fn too_deep() -> String {
    "RangeError: Maximum call stack size exceeded".into()
}

/// Where [`Heap::alloc_grammar`] puts a value it made.
enum Slot {
    Root,
    Element(Id, u32),
    Prop(Id, String),
    Proto(Id),
}

/// The objects of one parse.
#[derive(Debug, Default)]
pub(super) struct Heap {
    objects: Vec<HObject>,
}

impl Heap {
    pub(super) fn new() -> Heap {
        Heap::default()
    }

    /// A new, empty object of `class`.
    pub(super) fn alloc(&mut self, class: Class) -> Id {
        let id = Id(u32::try_from(self.objects.len()).expect("fewer than 2^32 objects"));
        self.objects.push(HObject {
            class,
            props: JsObject::new(),
            elements: BTreeMap::new(),
            length: 0,
            proto: HProto::Default,
        });
        id
    }

    fn object(&self, id: Id) -> &HObject {
        &self.objects[id.0 as usize]
    }

    fn object_mut(&mut self, id: Id) -> &mut HObject {
        &mut self.objects[id.0 as usize]
    }

    /// `CreateDataProperty(object, key, value)` on an object being built: an array's index
    /// is an element, any other key an own property (kept in its place when the object has
    /// it).
    pub(super) fn define(&mut self, id: Id, key: String, value: HValue) {
        let object = self.object_mut(id);
        if object.class == Class::Array
            && let Some(i) = array_index(&key)
        {
            object.elements.insert(i, value);
            object.length = object.length.max(i + 1);
            return;
        }
        object.props.insert(key, value);
    }

    /// A new array of these elements.
    pub(super) fn array(&mut self, items: Vec<HValue>) -> HValue {
        let id = self.alloc(Class::Array);
        let object = self.object_mut(id);
        object.length = u32::try_from(items.len()).expect("an array of at most 2^32 - 1");
        object.elements = items
            .into_iter()
            .enumerate()
            .map(|(i, v)| (i as u32, v))
            .collect();
        HValue::Object(id)
    }

    // -----------------------------------------------------------------------------------
    // [[Get]]
    // -----------------------------------------------------------------------------------

    /// `value`'s own property `key` (of `Object(value)` for a primitive).
    fn own(&self, value: &HValue, key: &str) -> Option<HValue> {
        match value {
            HValue::Undefined | HValue::Null | HValue::Bool(_) | HValue::Number(_) => None,
            HValue::String(s) => {
                if key == "length" {
                    return Some(HValue::Number(s.encode_utf16().count() as f64));
                }
                let i = array_index(key)? as usize;
                s.encode_utf16()
                    .nth(i)
                    .map(|unit| HValue::String(String::from_utf16_lossy(&[unit])))
            }
            HValue::Object(id) => {
                let object = self.object(*id);
                if object.class == Class::Array {
                    if key == "length" {
                        return Some(HValue::Number(f64::from(object.length)));
                    }
                    if let Some(i) = array_index(key) {
                        return object.elements.get(&i).cloned();
                    }
                }
                object.props.get(key).cloned()
            }
        }
    }

    /// Does `value` have the own property `key`?
    fn has_own(&self, value: &HValue, key: &str) -> bool {
        match value {
            HValue::String(s) => {
                key == "length"
                    || array_index(key).is_some_and(|i| (i as usize) < s.encode_utf16().count())
            }
            HValue::Object(id) => {
                let object = self.object(*id);
                if object.class == Class::Array {
                    if key == "length" {
                        return true;
                    }
                    if let Some(i) = array_index(key) {
                        return object.elements.contains_key(&i);
                    }
                }
                object.props.contains_key(key)
            }
            _ => false,
        }
    }

    /// The prototype of `Object(value)`: `None` for `null` and `undefined`; `Err(builtin)`
    /// for a built-in prototype, `Ok(Some(id))` for an object, `Ok(None)` for `null`.
    fn proto_of(&self, value: &HValue) -> Option<Result<Option<Id>, BuiltinProto>> {
        Some(match value {
            HValue::Undefined | HValue::Null => return None,
            HValue::Bool(_) => Err(BuiltinProto::BOOLEAN_PROTOTYPE),
            HValue::Number(_) => Err(BuiltinProto::NUMBER_PROTOTYPE),
            HValue::String(_) => Err(BuiltinProto::STRING_PROTOTYPE),
            HValue::Object(id) => {
                let object = self.object(*id);
                match object.proto {
                    HProto::Default => Err(object.class.builtin()),
                    HProto::Null => Ok(None),
                    HProto::Object(p) => Ok(Some(p)),
                }
            }
        })
    }

    /// `value[key]` (not `null` or `undefined`): the chain walked to a property or a built-in
    /// prototype.
    pub(super) fn lookup(&self, value: &HValue, key: &str) -> HFound {
        if let Some(v) = self.own(value, key) {
            return HFound::Value(v);
        }
        let mut proto = self.proto_of(value);
        loop {
            match proto {
                None | Some(Ok(None)) => return HFound::Value(HValue::Undefined),
                Some(Err(builtin)) => return HFound::Builtin(realm::intrinsic(builtin, key)),
                Some(Ok(Some(id))) => {
                    let at = HValue::Object(id);
                    if let Some(v) = self.own(&at, key) {
                        return HFound::Value(v);
                    }
                    proto = self.proto_of(&at);
                }
            }
        }
    }

    // -----------------------------------------------------------------------------------
    // [[Set]]
    // -----------------------------------------------------------------------------------

    /// Does `[[Set]]` of `__proto__` on an object with this prototype reach
    /// `Object.prototype`'s accessor (rather than an own `__proto__` data property of an
    /// object on the chain, or a chain ending in `null`)?
    fn reaches_proto_setter(&self, mut proto: Option<Result<Option<Id>, BuiltinProto>>) -> bool {
        loop {
            match proto {
                None | Some(Ok(None)) => return false,
                Some(Err(builtin)) => {
                    return realm::intrinsic(builtin, "__proto__") == Intrinsic::ProtoAccessor;
                }
                Some(Ok(Some(id))) => {
                    let at = HValue::Object(id);
                    if self.has_own(&at, "__proto__") {
                        return false;
                    }
                    proto = self.proto_of(&at);
                }
            }
        }
    }

    /// `target[key] = value` in strict-mode code (upstream's dist is a module): JavaScript's
    /// `[[Set]]`. On an object: an own property is set in its place (an array's element, or
    /// its `length`: `ArraySetLength`, `RangeError: Invalid array length` for a value that is
    /// no length, holes past the end); otherwise the chain decides, and every property an
    /// object here can inherit is a writable data property (an own one is created) except
    /// `Object.prototype.__proto__`, whose setter makes an object or `null` the prototype and
    /// ignores anything else. On a primitive: V8's `TypeError` (a string's `length` and indices
    /// are read-only, any other property cannot be created), except `__proto__` (the setter
    /// does nothing). `null` and `undefined` have no properties.
    pub(super) fn set(&mut self, target: &HValue, key: &str, value: HValue) -> Result<(), String> {
        let id = match target {
            HValue::Object(id) => *id,
            HValue::Undefined | HValue::Null => {
                return Err(format!(
                    "TypeError: Cannot set properties of {} (setting '{key}')",
                    if matches!(target, HValue::Null) {
                        "null"
                    } else {
                        "undefined"
                    }
                ));
            }
            primitive => {
                if key == "__proto__" {
                    return Ok(());
                }
                return Err(self.primitive_assignment_error(key, primitive));
            }
        };
        let class = self.object(id).class;
        // An own property: set in its place.
        if class == Class::Array && key == "length" {
            return self.set_length(id, &value);
        }
        if self.has_own(target, key) {
            self.define(id, key.to_string(), value);
            return Ok(());
        }
        if key == "__proto__" && self.reaches_proto_setter(self.proto_of(target)) {
            match &value {
                HValue::Null => self.object_mut(id).proto = HProto::Null,
                HValue::Object(p) => {
                    if self.on_chain(*p, id) {
                        return Err("TypeError: Cyclic __proto__ value".into());
                    }
                    self.object_mut(id).proto = HProto::Object(*p);
                }
                _ => {}
            }
            return Ok(());
        }
        self.define(id, key.to_string(), value);
        Ok(())
    }

    /// Is `id` on `from`'s prototype chain (`from` itself included)?
    fn on_chain(&self, from: Id, id: Id) -> bool {
        let mut at = Some(from);
        while let Some(p) = at {
            if p == id {
                return true;
            }
            at = match self.object(p).proto {
                HProto::Object(q) => Some(q),
                _ => None,
            };
        }
        false
    }

    /// `array.length = value`: `ArraySetLength`.
    fn set_length(&mut self, id: Id, value: &HValue) -> Result<(), String> {
        let number = self.to_number(value)?;
        let length = number as u32;
        if f64::from(length) != number {
            return Err("RangeError: Invalid array length".into());
        }
        let object = self.object_mut(id);
        if length < object.length {
            object.elements.split_off(&length);
        }
        object.length = length;
        Ok(())
    }

    /// `ToNumber(value)` (an object by its `ToPrimitive`, through the value it is).
    fn to_number(&self, value: &HValue) -> Result<f64, String> {
        Ok(match value {
            HValue::Undefined => f64::NAN,
            HValue::Null => 0.0,
            HValue::Bool(b) => f64::from(u8::from(*b)),
            HValue::Number(n) => *n,
            HValue::String(s) => js::string_to_number(&js::to_utf16(s)),
            HValue::Object(_) => value::to_number(&self.to_value(value))?,
        })
    }

    /// The `TypeError` strict-mode JS (V8) throws for `primitive[name] = value`: a string's
    /// `length` and its indices (UTF-16 units) are read-only properties of the string; any
    /// other name on a string, and any name on a number or a boolean, is a property that
    /// cannot be created. The value is shown whole, as `String()` writes it.
    fn primitive_assignment_error(&self, name: &str, primitive: &HValue) -> String {
        let (kind, shown) = match primitive {
            HValue::String(s) => {
                let index =
                    array_index(name).is_some_and(|i| (i as usize) < s.encode_utf16().count());
                if name == "length" || index {
                    return format!(
                        "TypeError: Cannot assign to read only property '{name}' of string '{s}'"
                    );
                }
                ("string", s.clone())
            }
            HValue::Number(n) => ("number", js::number_to_string(*n)),
            HValue::Bool(b) => ("boolean", b.to_string()),
            _ => unreachable!("an object or nullish is handled by the caller"),
        };
        format!("TypeError: Cannot create property '{name}' on {kind} '{shown}'")
    }

    /// `ToPropertyKey(value)`: a string as it is, anything else as `String()` makes it (an
    /// object through the value it is, which can throw).
    pub(super) fn to_property_key(&self, value: &HValue) -> Result<String, String> {
        Ok(match value {
            HValue::String(s) => s.clone(),
            HValue::Undefined => "undefined".into(),
            HValue::Null => "null".into(),
            HValue::Bool(b) => b.to_string(),
            HValue::Number(n) => js::number_to_string(*n),
            HValue::Object(_) => value::to_property_key(&self.to_value(value))?,
        })
    }

    // -----------------------------------------------------------------------------------
    // From the grammar, from JSON, from the AST
    // -----------------------------------------------------------------------------------

    /// A value of the tag grammar, as the JavaScript objects it made (a hash's or a call's
    /// parameters' prototype included). Where the stack cannot grow
    /// ([`crate::depth::can_grow`]), deeper than [`super::MAX_VALUE_DEPTH`] (an array, object,
    /// variable or call a level, its members one deeper) is `RangeError` (the grammar's own
    /// fixed limit keeps the values it makes within it); where it grows, any depth (the
    /// grammar's depth budget bounds what it makes, and [`Heap::snapshot`] recurses a level at a
    /// time on a stack with room for it).
    pub(super) fn alloc_grammar(&mut self, value: &grammar::Value) -> Result<HValue, String> {
        let limit = if crate::depth::can_grow() {
            usize::MAX
        } else {
            super::MAX_VALUE_DEPTH
        };
        let mut out = HValue::Undefined;
        let mut work: Vec<(&grammar::Value, Slot, usize)> = vec![(value, Slot::Root, 0)];
        while let Some((v, slot, depth)) = work.pop() {
            let made = match v {
                grammar::Value::Null => HValue::Null,
                grammar::Value::Bool(b) => HValue::Bool(*b),
                grammar::Value::Number(n) => HValue::Number(*n),
                grammar::Value::String(s) => HValue::String(s.clone()),
                container => {
                    let depth = depth + 1;
                    if depth > limit {
                        return Err(too_deep());
                    }
                    match container {
                        grammar::Value::Array(items) => {
                            let id = self.alloc(Class::Array);
                            self.object_mut(id).length =
                                u32::try_from(items.len()).map_err(|_| too_deep())?;
                            for (i, item) in items.iter().enumerate().rev() {
                                work.push((item, Slot::Element(id, i as u32), depth));
                            }
                            HValue::Object(id)
                        }
                        grammar::Value::Object(o) => {
                            let id = self.alloc(Class::Object);
                            self.queue_object(id, o, depth, &mut work);
                            HValue::Object(id)
                        }
                        grammar::Value::Variable(var) => {
                            let id = self.alloc(Class::Variable);
                            self.define(id, "$$mdtype".into(), HValue::String("Variable".into()));
                            let path = self.alloc(Class::Array);
                            self.object_mut(path).length =
                                u32::try_from(var.path.len()).map_err(|_| too_deep())?;
                            self.define(id, "path".into(), HValue::Object(path));
                            for (i, step) in var.path.iter().enumerate().rev() {
                                work.push((step, Slot::Element(path, i as u32), depth));
                            }
                            HValue::Object(id)
                        }
                        grammar::Value::Function(f) => {
                            let id = self.alloc(Class::Function);
                            self.define(id, "$$mdtype".into(), HValue::String("Function".into()));
                            self.define(id, "name".into(), HValue::String(f.name.clone()));
                            let parameters = self.alloc(Class::Object);
                            self.define(id, "parameters".into(), HValue::Object(parameters));
                            self.queue_object(parameters, &f.parameters, depth, &mut work);
                            HValue::Object(id)
                        }
                        _ => unreachable!("a primitive is handled above"),
                    }
                }
            };
            match slot {
                Slot::Root => out = made,
                Slot::Element(id, i) => {
                    self.object_mut(id).elements.insert(i, made);
                }
                Slot::Prop(id, key) => self.define(id, key, made),
                Slot::Proto(id) => {
                    self.object_mut(id).proto = match made {
                        HValue::Null => HProto::Null,
                        HValue::Object(p) => HProto::Object(p),
                        _ => HProto::Default,
                    };
                }
            }
        }
        Ok(out)
    }

    /// Queues a grammar object's properties (in its order: placeholders keep it whatever
    /// order the values are converted in) and its prototype.
    fn queue_object<'g>(
        &mut self,
        id: Id,
        o: &'g grammar::Object,
        depth: usize,
        work: &mut Vec<(&'g grammar::Value, Slot, usize)>,
    ) {
        match o.prototype() {
            grammar::Prototype::ObjectPrototype => {}
            grammar::Prototype::Null => self.object_mut(id).proto = HProto::Null,
            // A grammar object's prototype is the value its `__proto__` key held, one of
            // its members as deep as they are.
            grammar::Prototype::Value(p) => work.push((p, Slot::Proto(id), depth)),
        }
        let entries = o.entries();
        for (k, _) in &entries {
            self.define(id, (*k).to_string(), HValue::Undefined);
        }
        for (k, v) in entries.into_iter().rev() {
            work.push((v, Slot::Prop(id, k.to_string()), depth));
        }
    }

    /// A `meta` value of a hand-made token, read from JSON as upstream's harness reads it:
    /// `JSON.parse` (an own `__proto__` key is an own property), with `Ast.fromJSON`'s reviver
    /// (`Object.assign(new klass(), value)` for an object whose `$$mdtype` names one of
    /// `Function`, `Node` and `Variable`: an own `__proto__` key then sets the instance's
    /// prototype, as `Object.assign` assigns it). A `$$mdtype` that names a property every
    /// object inherits (`toString`, `__proto__`, ...) makes `new klass()` throw `TypeError:
    /// klass is not a constructor`; one that names `constructor` copies the object as it is.
    /// (The parser checks the JSON's depth first.)
    pub(super) fn alloc_json(&mut self, json: &serde_json::Value) -> Result<HValue, String> {
        use serde_json::Value as J;
        /// A container whose members are being converted: the JSON, how far, and the
        /// converted members.
        enum Frame<'j> {
            Array(std::slice::Iter<'j, J>, Vec<HValue>),
            Object(
                serde_json::map::Iter<'j>,
                Vec<(String, HValue)>,
                Option<&'j String>,
            ),
        }
        let leaf = |json: &J| -> Option<HValue> {
            Some(match json {
                J::Null => HValue::Null,
                J::Bool(b) => HValue::Bool(*b),
                J::Number(n) => HValue::Number(n.as_f64().unwrap_or(f64::NAN)),
                J::String(s) => HValue::String(s.clone()),
                J::Array(_) | J::Object(_) => return None,
            })
        };
        fn open(json: &J) -> Frame<'_> {
            match json {
                J::Array(items) => Frame::Array(items.iter(), Vec::with_capacity(items.len())),
                J::Object(o) => Frame::Object(o.iter(), Vec::with_capacity(o.len()), None),
                _ => unreachable!("a leaf"),
            }
        }
        if let Some(v) = leaf(json) {
            return Ok(v);
        }
        let mut stack = vec![open(json)];
        loop {
            let top = stack.last_mut().expect("a frame");
            let next = match top {
                Frame::Array(items, _) => items.next(),
                Frame::Object(entries, _, key) => entries.next().map(|(k, v)| {
                    *key = Some(k);
                    v
                }),
            };
            let done = match next {
                Some(json) => match leaf(json) {
                    Some(v) => v,
                    None => {
                        stack.push(open(json));
                        continue;
                    }
                },
                None => match stack.pop().expect("a frame") {
                    Frame::Array(_, items) => self.array(items),
                    Frame::Object(_, members, _) => self.revive(members)?,
                },
            };
            match stack.last_mut() {
                None => return Ok(done),
                Some(Frame::Array(_, items)) => items.push(done),
                Some(Frame::Object(_, members, key)) => {
                    members.push((key.take().expect("the key").clone(), done));
                }
            }
        }
    }

    /// A JSON object (its members converted, in the JSON's order, which `JSON.parse` sets
    /// with `CreateDataProperty`), then `Ast.fromJSON`'s reviver on it.
    fn revive(&mut self, members: Vec<(String, HValue)>) -> Result<HValue, String> {
        let parsed = self.alloc(Class::Object);
        for (k, v) in members {
            self.define(parsed, k, v);
        }
        let parsed_value = HValue::Object(parsed);
        // `AstTypes[value.$$mdtype]`, `$$mdtype` made a key.
        let mdtype = match self.own(&parsed_value, "$$mdtype") {
            None | Some(HValue::Undefined) => return Ok(parsed_value),
            Some(t) => self.to_property_key(&t)?,
        };
        let class = match mdtype.as_str() {
            "Function" => Class::Function,
            "Node" => Class::Node,
            "Variable" => Class::Variable,
            // `AstTypes.constructor` is `Object`: `Object.assign(new Object(), value)`.
            "constructor" => Class::Object,
            name if realm::has_inherited(BuiltinProto::OBJECT_PROTOTYPE, name) => {
                return Err("TypeError: klass is not a constructor".into());
            }
            _ => return Ok(parsed_value),
        };
        // `new klass()`, then `Object.assign` of the parsed object's own properties.
        let instance = self.alloc(class);
        match class {
            Class::Variable => {
                self.define(
                    instance,
                    "$$mdtype".into(),
                    HValue::String("Variable".into()),
                );
                let path = self.array(Vec::new());
                self.define(instance, "path".into(), path);
            }
            Class::Function => {
                self.define(
                    instance,
                    "$$mdtype".into(),
                    HValue::String("Function".into()),
                );
                self.define(instance, "name".into(), HValue::Undefined);
                self.define(instance, "parameters".into(), HValue::Undefined);
            }
            Class::Node => self.new_node(instance),
            Class::Object | Class::Array => {}
        }
        let instance_value = HValue::Object(instance);
        let entries: Vec<(String, HValue)> = self
            .object(parsed)
            .props
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (k, v) in entries {
            self.set(&instance_value, &k, v)?;
        }
        Ok(instance_value)
    }

    /// `new Node()`'s own properties: `$$mdtype`, `errors`, `lines`, `inline`, `attributes`,
    /// `children`, `type`, `tag`, `annotations`, `slots`.
    fn new_node(&mut self, id: Id) {
        self.define(id, "$$mdtype".into(), HValue::String("Node".into()));
        let errors = self.array(Vec::new());
        self.define(id, "errors".into(), errors);
        let lines = self.array(Vec::new());
        self.define(id, "lines".into(), lines);
        self.define(id, "inline".into(), HValue::Bool(false));
        let attributes = self.alloc(Class::Object);
        self.define(id, "attributes".into(), HValue::Object(attributes));
        let children = self.array(Vec::new());
        self.define(id, "children".into(), children);
        self.define(id, "type".into(), HValue::String("node".into()));
        self.define(id, "tag".into(), HValue::Undefined);
        let annotations = self.array(Vec::new());
        self.define(id, "annotations".into(), annotations);
        let slots = self.alloc(Class::Object);
        self.define(id, "slots".into(), HValue::Object(slots));
    }

    /// An AST value as the object it is (what [`Heap::snapshot`] made of one).
    pub(super) fn alloc_scalar(&mut self, scalar: &Scalar) -> HValue {
        match scalar {
            Scalar::Undefined => HValue::Undefined,
            Scalar::Null => HValue::Null,
            Scalar::Boolean(b) => HValue::Bool(*b),
            Scalar::Number(n) => HValue::Number(*n),
            Scalar::String(s) => HValue::String(s.clone()),
            Scalar::Array(items) => {
                let items = items.iter().map(|v| self.alloc_scalar(v)).collect();
                self.array(items)
            }
            Scalar::Object(o) => {
                let id = self.alloc(Class::Object);
                for (k, v) in o {
                    let v = self.alloc_scalar(v);
                    self.define(id, k.clone(), v);
                }
                HValue::Object(id)
            }
            // What the parser's own attributes hold: never a variable, a call, a tag or an
            // object of another kind when a node is annotated (see `handle_attrs`).
            other => self.alloc_value(&Value::from_scalar(other)),
        }
    }

    /// A value as an object here (a copy: nothing is shared with it afterwards).
    fn alloc_value(&mut self, value: &Value) -> HValue {
        match value {
            Value::Undefined => HValue::Undefined,
            Value::Null => HValue::Null,
            Value::Boolean(b) => HValue::Bool(*b),
            Value::Number(n) => HValue::Number(*n),
            Value::String(s) => HValue::String(s.to_string_lossy().into_owned()),
            other => {
                let class = match value::prototype_of(other) {
                    Some(Proto::Builtin(BuiltinProto::ARRAY_PROTOTYPE))
                        if value::is_array(other) =>
                    {
                        Class::Array
                    }
                    Some(Proto::Builtin(BuiltinProto::VARIABLE_PROTOTYPE)) => Class::Variable,
                    Some(Proto::Builtin(BuiltinProto::AST_FUNCTION_PROTOTYPE)) => Class::Function,
                    Some(Proto::Builtin(BuiltinProto::NODE_PROTOTYPE)) => Class::Node,
                    _ => Class::Object,
                };
                let id = self.alloc(class);
                for (k, v) in value::own_entries(other) {
                    let v = self.alloc_value(&v);
                    self.define(id, k, v);
                }
                if let Value::Array(items) = other {
                    self.object_mut(id).length = items.len() as u32;
                }
                if let Value::Object(o) = other {
                    self.object_mut(id).proto = match o.proto() {
                        Proto::Builtin(_) => HProto::Default,
                        Proto::Null => HProto::Null,
                        Proto::Value(p) => match self.alloc_value(p) {
                            HValue::Object(p) => HProto::Object(p),
                            _ => HProto::Default,
                        },
                    };
                }
                HValue::Object(id)
            }
        }
    }

    // -----------------------------------------------------------------------------------
    // The AST's values
    // -----------------------------------------------------------------------------------

    /// The AST's value of `value` (see the module's documentation): plain data as the plain
    /// [`Scalar`] variants, any other object as [`Scalar::Js`]. `memo` keeps each object's
    /// [`Value`], so that an object two references share is one value.
    pub(super) fn snapshot(&self, value: &HValue, memo: &mut Memo) -> Scalar {
        match value {
            HValue::Undefined => Scalar::Undefined,
            HValue::Null => Scalar::Null,
            HValue::Bool(b) => Scalar::Boolean(*b),
            HValue::Number(n) => Scalar::Number(*n),
            HValue::String(s) => Scalar::String(s.clone()),
            // (A level at a time on a stack with room for it: values nest as deep as the
            // grammar's depth budget lets them.)
            HValue::Object(id) => crate::depth::grow!(match self.plain(*id, memo) {
                Some(scalar) => scalar,
                None => Scalar::Js(crate::types::JsObjectValue::new(
                    self.to_value_memo(value, memo),
                )),
            }),
        }
    }

    /// The plain [`Scalar`] of an object, when one holds it (its members snapshotted in turn):
    /// an object or an array with its built-in prototype (an array without holes or other
    /// properties), a variable or a call as the grammar makes them (`{$$mdtype, path}` with a
    /// path of strings and numbers; `{$$mdtype, name, parameters}` with a string name and
    /// parameters that are a plain object).
    fn plain(&self, id: Id, memo: &mut Memo) -> Option<Scalar> {
        let object = self.object(id);
        if !matches!(object.proto, HProto::Default) {
            return None;
        }
        Some(match object.class {
            Class::Object => Scalar::Object(
                object
                    .props
                    .iter()
                    .map(|(k, v)| (k.clone(), self.snapshot(v, memo)))
                    .collect(),
            ),
            Class::Array => {
                if !object.props.is_empty() || object.elements.len() != object.length as usize {
                    return None;
                }
                Scalar::Array(
                    object
                        .elements
                        .values()
                        .map(|v| self.snapshot(v, memo))
                        .collect(),
                )
            }
            Class::Variable => {
                let mut props = object.props.iter();
                let path = match (props.next(), props.next(), props.next()) {
                    (Some((t, HValue::String(kind))), Some((p, HValue::Object(path))), None)
                        if t == "$$mdtype" && kind == "Variable" && p == "path" =>
                    {
                        self.object(*path)
                    }
                    _ => return None,
                };
                if path.class != Class::Array
                    || !matches!(path.proto, HProto::Default)
                    || !path.props.is_empty()
                    || path.elements.len() != path.length as usize
                {
                    return None;
                }
                let steps = path
                    .elements
                    .values()
                    .map(|step| match step {
                        HValue::Number(n) => Some(crate::ast::PathSegment::Number(*n)),
                        HValue::String(s) => Some(crate::ast::PathSegment::String(s.clone())),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()?;
                Scalar::Variable(crate::ast::Variable::new(steps))
            }
            Class::Function => {
                let mut props = object.props.iter();
                let (name, parameters) =
                    match (props.next(), props.next(), props.next(), props.next()) {
                        (
                            Some((t, HValue::String(kind))),
                            Some((n, HValue::String(name))),
                            Some((p, HValue::Object(parameters))),
                            None,
                        ) if t == "$$mdtype"
                            && kind == "Function"
                            && n == "name"
                            && p == "parameters" =>
                        {
                            (name, self.object(*parameters))
                        }
                        _ => return None,
                    };
                if parameters.class != Class::Object || !matches!(parameters.proto, HProto::Default)
                {
                    return None;
                }
                let parameters: IndexMap<String, Scalar> = parameters
                    .props
                    .iter()
                    .map(|(k, v)| (k.clone(), self.snapshot(v, memo)))
                    .collect();
                Scalar::Function(crate::ast::Function::new(name.clone(), parameters))
            }
            Class::Node => return None,
        })
    }

    /// The [`Value`] an object here is (a copy made once per object: see [`Memo`]).
    pub(super) fn to_value(&self, value: &HValue) -> Value {
        self.to_value_memo(value, &mut Memo::default())
    }

    fn to_value_memo(&self, value: &HValue, memo: &mut Memo) -> Value {
        let root = match value {
            HValue::Undefined => return Value::Undefined,
            HValue::Null => return Value::Null,
            HValue::Bool(b) => return Value::Boolean(*b),
            HValue::Number(n) => return Value::Number(*n),
            HValue::String(s) => return Value::string(s.as_str()),
            HValue::Object(id) => *id,
        };
        // Post-order over the objects not yet made: each made once its members and its
        // prototype are.
        let mut work = vec![(root, false)];
        while let Some((id, ready)) = work.pop() {
            if memo.values.contains_key(&id) {
                continue;
            }
            let object = self.object(id);
            if !ready {
                work.push((id, true));
                let pending = object
                    .props
                    .values()
                    .chain(object.elements.values())
                    .filter_map(|v| match v {
                        HValue::Object(c) if !memo.values.contains_key(c) => Some(*c),
                        _ => None,
                    })
                    .chain(match object.proto {
                        HProto::Object(p) if !memo.values.contains_key(&p) => Some(p),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                work.extend(pending.into_iter().map(|c| (c, false)));
                continue;
            }
            let member = |v: &HValue| -> Value {
                match v {
                    HValue::Undefined => Value::Undefined,
                    HValue::Null => Value::Null,
                    HValue::Bool(b) => Value::Boolean(*b),
                    HValue::Number(n) => Value::Number(*n),
                    HValue::String(s) => Value::string(s.as_str()),
                    HValue::Object(c) => memo.values[c].clone(),
                }
            };
            let props: JsObject<Value> = object
                .props
                .iter()
                .map(|(k, v)| (k.clone(), member(v)))
                .collect();
            let made = match object.class {
                Class::Array => Value::Array(std::sync::Arc::new(Array::from_parts(
                    object.length,
                    object.elements.iter().map(|(i, v)| (*i, member(v))),
                    props.into(),
                ))),
                class => {
                    let proto = match object.proto {
                        HProto::Default => Proto::Builtin(class.builtin()),
                        HProto::Null => Proto::Null,
                        HProto::Object(p) => Proto::Value(memo.values[&p].clone()),
                    };
                    Value::Object(std::sync::Arc::new(Object::with_proto(props, proto)))
                }
            };
            memo.values.insert(id, made);
        }
        memo.values[&root].clone()
    }

    /// An attributes object's own properties and prototype, as a node holds them.
    pub(super) fn snapshot_attributes(
        &self,
        id: Id,
        memo: &mut Memo,
    ) -> (IndexMap<String, Scalar>, Option<Box<Scalar>>) {
        let object = self.object(id);
        let attributes = object
            .props
            .iter()
            .map(|(k, v)| (k.clone(), self.snapshot(v, memo)))
            .collect();
        let proto = match object.proto {
            HProto::Default => None,
            HProto::Null => Some(Box::new(Scalar::Null)),
            HProto::Object(p) => Some(Box::new(self.snapshot(&HValue::Object(p), memo))),
        };
        (attributes, proto)
    }

    /// A hand-made annotation that is an attribute object as the grammar makes one: a plain
    /// object whose own properties are exactly `type` and `name` (strings) and maybe `value`,
    /// in that order. Its type, name and value.
    pub(super) fn attribute_object(&self, value: &HValue) -> Option<(String, String, HValue)> {
        let HValue::Object(id) = value else {
            return None;
        };
        let object = self.object(*id);
        if object.class != Class::Object || !matches!(object.proto, HProto::Default) {
            return None;
        }
        let mut props = object.props.iter();
        match (props.next(), props.next(), props.next(), props.next()) {
            (Some((t, HValue::String(kind))), Some((n, HValue::String(name))), value, None)
                if t == "type" && n == "name" && value.is_none_or(|(v, _)| v == "value") =>
            {
                Some((
                    kind.clone(),
                    name.clone(),
                    value.map_or(HValue::Undefined, |(_, v)| v.clone()),
                ))
            }
            _ => None,
        }
    }

    /// The text `value` is in an error message (`${value}`): `String(value)`.
    pub(super) fn display(&self, value: &HValue) -> Result<String, String> {
        match value {
            HValue::Object(_) => {
                Ok(value::to_js_string(&self.to_value(value))?.into_string_lossy())
            }
            other => self.to_property_key(other),
        }
    }
}

/// The [`Value`] [`Heap::snapshot`] made of each object so far.
#[derive(Default)]
pub(super) struct Memo {
    values: HashMap<Id, Value>,
}
