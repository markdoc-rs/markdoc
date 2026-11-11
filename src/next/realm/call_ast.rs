//! Upstream's AST classes' methods (`Node`, `Variable`, `Function` of `src/ast/`), and the
//! generator and iterator built-ins `Node.prototype.walk` (a generator method) reaches,
//! called (see [`super::call`]).
//!
//! A value is an instance of an AST class only where the object model makes one (a resolved
//! `Node` in an attribute of a hand-made AST); a document then reads these methods off it and
//! can hand one to `ToPrimitive` or `JSON.stringify` under `toString`, `valueOf` or `toJSON`,
//! with any object as `this`. Each runs here as upstream's dist runs it: the same property
//! reads, in the same order, with V8's errors where one fails (`Cannot read properties of
//! undefined (reading 'map')`, `this.path.reduce is not a function`, ...).

// Each arm of a dispatch `match` runs in a closure of its own, called where it is made, so
// that its locals have a frame of their own: a debug build gives a function one frame for
// every arm's locals, which a nested conversion keeps live at each level of a recursion.
#![allow(clippy::redundant_closure_call)]

use super::call::no_such_function;
use super::call::{Called, argument as arg, done, opaque, opaque_function, type_err};
use super::{Builtin, is_object};
use crate::next::schema::value::{self, PlainObject, Value, get_property};

type R<T> = Result<T, String>;

/// V8's `TypeError` for calling `value`, a non-function the dist reaches as `text` (V8 prints
/// the call's source text): `this.path.reduce is not a function`.
fn not_callable(text: &str) -> String {
    type_err(format!("{text} is not a function"))
}

/// `value[key]` where the dist reads it (V8's `Cannot read properties of undefined (reading
/// 'map')` for `undefined` and `null`).
fn read(value: &Value, key: &str) -> R<Value> {
    get_property(value, key)
}

/// `value.method(...args)` as the dist calls it, `text` the call's source text for V8's error
/// when the property is no function.
fn invoke(value: &Value, method: &str, args: &[Value], text: &str) -> R<Called> {
    let f = read(value, method)?;
    match f {
        Value::Builtin(b) if b.is_callable() => super::call::call(b, value, args),
        _ => Err(not_callable(text)),
    }
}

/// The value a call made, as a value (a new primitive wrapper as the primitive it holds:
/// only `JSON.stringify` tells them apart, and no caller here writes one).
fn plain(called: Called) -> Value {
    match called {
        Called::Value(v) | Called::Wrapper(v) => v,
    }
}

/// `transformer.findSchema(node, {nodes = {}, tags = {}} = {})`: `node.tag ? tags[node.tag] :
/// nodes[node.type]`, where the config is never one with `nodes` or `tags` here (a call passes
/// no argument or a string), so both are `{}`: a name every object inherits, or `undefined`.
fn find_schema(node: &Value) -> R<Value> {
    let tag = read(node, "tag")?;
    let name = if tag.to_boolean() {
        tag
    } else {
        read(node, "type")?
    };
    let key = super::call::property_key(&name)?;
    Ok(crate::next::schema::inherited(&key))
}

/// `transformer.node(node, config)` with a config that has no schemas: the schema's
/// `transform` (none of the inherited names has one), else `transformer.children(node)`, which
/// is what it returns (no inherited schema has a `render`).
fn transform_node(node: &Value) -> R<Value> {
    let _ = find_schema(node)?;
    transform_children(node)
}

/// `transformer.children(node, config)`: `node.children.flatMap((child) => this.node(child,
/// config))`, then `children.some(isPromise)` (no item is a thenable: every one is the
/// flattened result of a nested `children`).
fn transform_children(node: &Value) -> R<Value> {
    let children = read(node, "children")?;
    let flat_map = read(&children, "flatMap")?;
    match flat_map {
        Value::Builtin(Builtin::ARRAY_PROTOTYPE_FLAT_MAP) => {}
        _ => return Err(not_callable("node2.children.flatMap")),
    }
    // `Array.prototype.flatMap` over an array-like: each present index's item through the
    // callback, an array result spread one level.
    let len = super::call::len_of(&children)?;
    let mut out = Vec::new();
    for k in super::call::indices(&children, len) {
        let child = read(&children, &super::call::key_of(k))?;
        match transform_node(&child)? {
            // A hole is no element (`FlattenIntoArray` asks `HasProperty`).
            Value::Array(items) => out.extend(items.present().map(|(_, v)| v.clone())),
            other if value::is_array(&other) => {}
            other => out.push(other),
        }
    }
    for item in &out {
        if crate::next::realm::is_object(item) {
            // `isPromise(a)`: `typeof a.then === 'function'`.
            read(item, "then")?;
        }
    }
    Ok(Value::array(out))
}

/// `transformer.attributes(node, config)` with a config that has no schemas: the global
/// attributes `class` (through `Class.transform`) and `id`, read off `node.attributes`.
fn transform_attributes(node: &Value) -> R<Value> {
    let schema = find_schema(node)?;
    // `{...globalAttributes, ...schema.attributes}`: an inherited name's value has no
    // `attributes`.
    let _ = schema;
    let mut output = PlainObject::new();
    for key in ["class", "id"] {
        let attributes = read(node, "attributes")?;
        let mut value = read(&attributes, key)?;
        if key == "class" {
            value = crate::next::schema::class_transform(value);
        }
        if value.is_undefined() {
            continue;
        }
        output.assign(key, value);
    }
    Ok(Value::object(output.into_properties()))
}

/// Upstream's `resolve(value, config)` (`ast/base.ts`) of a value a call reaches: `null`,
/// `undefined` and anything `typeof` does not call an object as it is; an array's items
/// resolved (`value.map`); an AST object (a truthy `$$mdtype`) whose `resolve` is a function
/// resolved by it; any other object that is not a plain object as it is; a plain object's
/// entries resolved into a new one.
fn resolve_value(v: &Value, config: &Value) -> R<Value> {
    if v.is_nullish() || super::call::type_of(v) != "object" {
        return Ok(v.clone());
    }
    if let Value::Array(items) = v {
        // `map` keeps the holes.
        let mut present = Vec::with_capacity(items.present_len());
        for (i, item) in items.present() {
            present.push((i, resolve_value(item, config)?));
        }
        return Ok(Value::sparse_array(
            items.len() as u32,
            present,
            indexmap::IndexMap::new(),
        ));
    }
    if value::is_array(v) {
        // `Array.prototype` itself: `map` over no items.
        return Ok(Value::array(Vec::new()));
    }
    if read(v, "$$mdtype")?.to_boolean()
        && let Value::Builtin(f) = read(v, "resolve")?
        && f.is_callable()
    {
        return Ok(plain(super::call::call(
            f,
            v,
            std::slice::from_ref(config),
        )?));
    }
    if !matches!(
        crate::next::realm::prototype_of(v),
        Value::Builtin(Builtin::OBJECT_PROTOTYPE)
    ) {
        return Ok(v.clone());
    }
    let mut output = PlainObject::new();
    for (k, item) in value::own_entries(v) {
        output.assign(k, resolve_value(&item, config)?);
    }
    Ok(Value::object(output.into_properties()))
}

/// `Node.prototype.resolve` with `this`: `Object.assign(new Node(), this, {children:
/// this.children.map((child) => child.resolve(config)), attributes:
/// resolve(this.attributes, config), slots: Object.fromEntries(Object.entries(this.slots)
/// .map(([name, slot]) => [name, slot.resolve(config)]))})`: a `Node`, which only
/// `JSON.stringify` reads (its own enumerable properties, in its order).
fn node_resolve(this: &Value, config: &Value) -> R<Value> {
    // `this.children.map(...)`.
    let children = read(this, "children")?;
    let map = read(&children, "map")?;
    if !matches!(map, Value::Builtin(Builtin::ARRAY_PROTOTYPE_MAP)) {
        return Err(not_callable("this.children.map"));
    }
    let len = super::call::len_of(&children)?;
    let mut resolved_children = vec![Value::Undefined; len as usize];
    for k in super::call::indices(&children, len) {
        let child = read(&children, &super::call::key_of(k))?;
        resolved_children[k as usize] = plain(invoke(
            &child,
            "resolve",
            std::slice::from_ref(config),
            "child.resolve",
        )?);
    }
    let attributes = resolve_value(&read(this, "attributes")?, config)?;
    // `Object.entries(this.slots)`.
    let slots = read(this, "slots")?;
    if slots.is_nullish() {
        return Err(type_err("Cannot convert undefined or null to object"));
    }
    let mut resolved_slots = crate::types::JsObject::new();
    for (name, slot) in value::own_entries(&slots) {
        let v = plain(invoke(
            &slot,
            "resolve",
            std::slice::from_ref(config),
            "slot2.resolve",
        )?);
        resolved_slots.insert(name, v);
    }
    // `new Node()`'s fields, then `this`'s own enumerable ones, then the three resolved.
    let mut node = PlainObject::new();
    node.assign("$$mdtype", Value::string("Node"));
    node.assign("errors", Value::array(Vec::new()));
    node.assign("lines", Value::array(Vec::new()));
    node.assign("inline", Value::Boolean(false));
    node.assign("attributes", Value::empty_object());
    node.assign("children", Value::array(Vec::new()));
    node.assign("type", Value::string("node"));
    node.assign("tag", Value::Undefined);
    node.assign("annotations", Value::array(Vec::new()));
    node.assign("slots", Value::empty_object());
    for (k, v) in value::own_entries(this) {
        node.assign(k, v);
    }
    node.assign("children", Value::array(resolved_children));
    node.assign("attributes", attributes);
    node.assign("slots", Value::object(resolved_slots));
    Ok(Value::object(node.into_properties()))
}

/// `Variable.prototype.resolve({variables} = {})` with `this`, where the argument is never an
/// object with `variables` (a call passes nothing or a string): `this.path.reduce((obj = {},
/// key) => obj[key], undefined)`.
fn variable_resolve(this: &Value) -> R<Value> {
    let path = read(this, "path")?;
    let reduce = read(&path, "reduce")?;
    if !matches!(reduce, Value::Builtin(Builtin::ARRAY_PROTOTYPE_REDUCE)) {
        return Err(not_callable("this.path.reduce"));
    }
    let len = super::call::len_of(&path)?;
    let mut obj = Value::Undefined;
    for k in super::call::indices(&path, len) {
        let key = read(&path, &super::call::key_of(k))?;
        if obj.is_undefined() {
            obj = Value::empty_object();
        }
        let key = super::call::property_key(&key)?;
        obj = read(&obj, &key)?;
    }
    Ok(obj)
}

/// The AST classes' methods.
pub(super) fn ast_function(f: Builtin, this: &Value, args: &[Value]) -> R<Called> {
    let a0 = arg(args, 0);
    match f {
        // A generator method: the call makes the generator object, running none of its body.
        Builtin::NODE_PROTOTYPE_WALK => (|| -> R<Called> { Ok(opaque()) })(),
        // `this.children.push(node)`; returns nothing.
        Builtin::NODE_PROTOTYPE_PUSH => (|| -> R<Called> {
            let children = read(this, "children")?;
            invoke(&children, "push", &[a0], "this.children.push")?;
            done(Value::Undefined)
        })(),
        Builtin::NODE_PROTOTYPE_RESOLVE => (|| -> R<Called> {
            let config = if a0.is_undefined() {
                Value::empty_object()
            } else {
                a0
            };
            done(node_resolve(this, &config)?)
        })(),
        Builtin::NODE_PROTOTYPE_FIND_SCHEMA => (|| -> R<Called> { done(find_schema(this)?) })(),
        Builtin::NODE_PROTOTYPE_TRANSFORM_ATTRIBUTES => {
            (|| -> R<Called> { done(transform_attributes(this)?) })()
        }
        Builtin::NODE_PROTOTYPE_TRANSFORM_CHILDREN => {
            (|| -> R<Called> { done(transform_children(this)?) })()
        }
        Builtin::NODE_PROTOTYPE_TRANSFORM => (|| -> R<Called> { done(transform_node(this)?) })(),
        Builtin::VARIABLE_PROTOTYPE_RESOLVE => (|| -> R<Called> {
            // `{variables} = arg`: destructuring `null` throws.
            if matches!(a0, Value::Null) {
                return Err(type_err(
                    "Cannot destructure property 'variables' of 'object null' as it is null.",
                ));
            }
            done(variable_resolve(this)?)
        })(),
        // `config?.functions?.[this.name]`: no argument a call passes has `functions`.
        Builtin::AST_FUNCTION_PROTOTYPE_RESOLVE => (|| -> R<Called> { done(Value::Null) })(),
        other => Err(no_such_function(other)),
    }
}

/// `GeneratorFunction`, `Generator.prototype`'s methods, `Iterator` and its helpers.
pub(super) fn iterator_function(f: Builtin, this: &Value, args: &[Value]) -> R<Called> {
    let a0 = arg(args, 0);
    let helper = |name: &str| format!("Iterator.prototype.{name}");
    match f {
        // CreateDynamicFunction: the arguments as text, compiled (see `Function`).
        Builtin::GENERATOR_FUNCTION => (|| -> R<Called> {
            for a in args {
                super::call::string(a)?;
            }
            Ok(opaque_function())
        })(),
        Builtin::GENERATOR_PROTOTYPE_NEXT
        | Builtin::GENERATOR_PROTOTYPE_RETURN
        | Builtin::GENERATOR_PROTOTYPE_THROW => {
            let name = match f {
                Builtin::GENERATOR_PROTOTYPE_NEXT => "next",
                Builtin::GENERATOR_PROTOTYPE_RETURN => "return",
                _ => "throw",
            };
            Err(type_err(format!(
                "Method [Generator].prototype.{name} called on incompatible receiver {}",
                super::call::no_side_effects_to_string(this)
            )))
        }
        Builtin::ITERATOR => Err(type_err("Constructor Iterator requires 'new'")),
        Builtin::ITERATOR_FROM => (|| -> R<Called> {
            match a0 {
                Value::String(_) => Ok(opaque()),
                v if is_object(&v) => Ok(opaque()),
                _ => Err(type_err("Iterator.from called on non-object")),
            }
        })(),
        Builtin::ITERATOR_PROTOTYPE_MAP
        | Builtin::ITERATOR_PROTOTYPE_FILTER
        | Builtin::ITERATOR_PROTOTYPE_FOR_EACH
        | Builtin::ITERATOR_PROTOTYPE_SOME
        | Builtin::ITERATOR_PROTOTYPE_EVERY
        | Builtin::ITERATOR_PROTOTYPE_FIND
        | Builtin::ITERATOR_PROTOTYPE_REDUCE => {
            // The callback (never a function here) is checked first.
            let name = f.name().rsplit('.').next().unwrap_or_default();
            Err(type_err(format!(
                "string \"{}\" is not a function",
                helper(name)
            )))
        }
        Builtin::ITERATOR_PROTOTYPE_FLAT_MAP => {
            (|| -> R<Called> { Err(type_err("Iterator.prototype.flatMap is not a function")) })()
        }
        Builtin::ITERATOR_PROTOTYPE_TAKE | Builtin::ITERATOR_PROTOTYPE_DROP => (|| -> R<Called> {
            let n = super::call::number(&a0)?;
            if n.is_nan() || super::call::integer(&a0)? < 0.0 {
                return Err(format!(
                    "RangeError: {} must be positive",
                    super::call::describe_primitive(&a0)
                ));
            }
            Ok(opaque())
        })(),
        // `IteratorStep`: `this.next()`.
        Builtin::ITERATOR_PROTOTYPE_TO_ARRAY => (|| -> R<Called> {
            let next = read(this, "next")?;
            match next {
                Value::Builtin(b) if b.is_callable() => {
                    let result = super::call::call(b, this, &[])?;
                    match plain(result) {
                        r if is_object(&r) => {
                            if read(&r, "done")?.to_boolean() {
                                done(Value::array(Vec::new()))
                            } else {
                                // `next` is a built-in called with the same `this` each
                                // time: it gives the same answer forever, and V8 collects
                                // values until its array length limit (node runs out of
                                // memory first).
                                Err("RangeError: Invalid array length".into())
                            }
                        }
                        r => Err(type_err(format!(
                            "Iterator result {} is not an object",
                            super::call::describe_primitive(&r)
                        ))),
                    }
                }
                other => Err(type_err(format!(
                    "{} is not a function",
                    super::call::default_call_site(&other)
                ))),
            }
        })(),
        other => Err(no_such_function(other)),
    }
}
