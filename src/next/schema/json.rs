//! A [`Config`] from JSON, read as `JSON.parse` makes the JavaScript object the JSON
//! describes, and used as upstream uses such an object.
//!
//! Owner: the semantics agent; JavaScript's reading of every shape: the model agent (A2).
//!
//! The JSON is upstream's config with two conventions for what JSON cannot hold (the ones
//! `tests/next/oracle.cjs` revives on the JavaScript side):
//!
//! - `{"$regex": source, "flags": flags}` is `new RegExp(source, flags)` (for `matches`);
//! - `{"$type": name}` is `TYPES[name]` wherever it is (a `type`, a `returns`, a `matches`, a
//!   default, a variable, ...): a constructor (`String`, `Number`, `Boolean`, `Object`,
//!   `Array`), one of upstream's classes (`Class`, `Id`, `ConditionalAttributeType`,
//!   `PartialFile`), what a plain object inherits under the name (`constructor` is `Object`,
//!   `toString` `Object.prototype.toString`, `__proto__` `Object.prototype`: a
//!   [`crate::next::realm`] built-in, [`TypeSpec::Realm`] as a type, [`Matches::Call`] as a
//!   `matches`), or `undefined` for any other name. A type given as a string (`"String"`) is
//!   upstream's other spelling; the two differ where upstream compares types by identity
//!   (`returns` against `type`, with `validation.validateFunctions`).
//!
//! Every value is read as `JSON.parse` makes it (a `__proto__` key is an own property, which
//! upstream's spreads copy as one: `{"tags": {"__proto__": {...}}}` is a tag named
//! `__proto__`) and with the JavaScript semantics upstream's code gives it: a map upstream
//! spreads or enumerates (`tags`, `nodes`, `functions`, a schema's `attributes` and `slots`,
//! a function's `parameters`) is `Object.entries` of whatever value it is (a string's
//! characters, an array's items, nothing for a number); a schema upstream reads fields of
//! that is not an object reads `undefined` for each, and throws for `null` where upstream
//! reads one ([`SchemaKind`]); `render`, `children` and `inline` keep their value
//! ([`Schema`]); an `errorLevel` is the level whatever it is; a `type` that is not a type is
//! [`TypeSpec::Value`]; a `validate` or a function's `transform` that is not a function (JSON
//! has none) throws V8's `TypeError` where upstream calls it, and a schema's `transform`,
//! which upstream calls only when it is a function, is never one.
//!
//! # Divergences from upstream
//!
//! - A RegExp as a value (`{"$regex": ...}` where upstream would hold the object as a
//!   value: in `variables`, `default`, `render`, an item of `matches`): `Err`. (A
//!   `{"$type": name}` is revived wherever it is, as the oracle revives it: see
//!   [`crate::next::realm::harness_type`].)
//! - `slots` or a function's `parameters` given as a string or an array: upstream's
//!   `schema.slots[key]` also finds its `length` (a slot or parameter named `length` is then
//!   defined), and a string's `String.prototype` methods; here only its items are entries.
//!   (`partials` given so has its `length` too; its methods are the realm's.)

use super::{
    BuiltinType, Config, ConfigFunction, FunctionTransformFn, FunctionValidateFn, JsRegExp,
    MarkdocType, Matches, Partial, Schema, SchemaAttribute, SchemaKind, SchemaSlot, TypeSpec,
    ValidateFn, Validation, Value, Variables,
};
use crate::types::{MarkdocError, ValidationLevel, is_array_index};
use indexmap::IndexMap;
use serde_json::Value as Json;
use std::borrow::Cow;
use std::sync::Arc;

type R<T> = std::result::Result<T, String>;

/// JS `ToBoolean` of a JSON value.
fn truthy(v: &Json) -> bool {
    match v {
        Json::Null => false,
        Json::Bool(b) => *b,
        Json::Number(n) => n.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Json::String(s) => !s.is_empty(),
        Json::Array(_) | Json::Object(_) => true,
    }
}

/// A value in a config, revived as the oracle revives it: `{"$type": name}` anywhere is
/// `TYPES[name]` ([`crate::next::realm::harness_type`]: a constructor, one of upstream's
/// classes, a name every object inherits, or `undefined`). A RegExp as a value is not
/// modeled.
fn value(v: &Json, at: &str) -> R<Value> {
    check_plain(v, at)?;
    Ok(Value::from_json_with(v, &|o| {
        o.get("$type")
            .and_then(Json::as_str)
            .map(crate::next::realm::harness_type)
    }))
}

fn check_plain(v: &Json, at: &str) -> R<()> {
    // Iterative: a config value can nest any depth.
    let mut stack = vec![v];
    while let Some(v) = stack.pop() {
        match v {
            Json::Array(items) => stack.extend(items),
            Json::Object(o) => {
                if o.get("$regex").is_some_and(Json::is_string) {
                    return Err(format!("not modeled: a RegExp as a value ({at})"));
                }
                if o.get("$type").is_some_and(Json::is_string) {
                    // Revived whole: its other keys are not read.
                    continue;
                }
                stack.extend(o.values());
            }
            _ => {}
        }
    }
    Ok(())
}

/// `Object.entries(value)` (and what a spread `{...value}` copies) of a JSON value: an
/// object's own properties in a JS object's order (array-index keys ascending, then the others
/// in the JSON's order; `__proto__` an own key like any other), an array's items by index, a
/// string's code units by index (each a one-unit string: only that it is a truthy string
/// matters where upstream reads one), nothing for a number, a boolean or `null`.
fn entries(v: &Json) -> Vec<(String, Cow<'_, Json>)> {
    match v {
        Json::Object(o) => {
            let (mut indices, mut others) = (Vec::new(), Vec::new());
            for (k, v) in o {
                if is_array_index(k) {
                    indices.push((k.clone(), Cow::Borrowed(v)));
                } else {
                    others.push((k.clone(), Cow::Borrowed(v)));
                }
            }
            indices.sort_by_key(|(k, _)| k.parse::<u64>().expect("an array index is a number"));
            indices.extend(others);
            indices
        }
        Json::Array(items) => items
            .iter()
            .enumerate()
            .map(|(i, v)| (i.to_string(), Cow::Borrowed(v)))
            .collect(),
        Json::String(s) => s
            .encode_utf16()
            .enumerate()
            .map(|(i, unit)| {
                (
                    i.to_string(),
                    Cow::Owned(Json::String(String::from_utf16_lossy(&[unit]))),
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// What a schema value that upstream reads fields of is.
fn kind(v: &Json) -> SchemaKind {
    match v {
        Json::Object(_) => SchemaKind::Object,
        Json::Null => SchemaKind::Null,
        _ if truthy(v) => SchemaKind::Truthy,
        _ => SchemaKind::Falsy,
    }
}

/// V8's `TypeError` for calling a value that is not a function (`schema.validate(...)`,
/// `fn.transform?.(...)`: V8 names the callee by the dist's source text).
fn not_a_function(expression: &'static str) -> MarkdocError {
    MarkdocError::ValidationError(format!("TypeError: {expression} is not a function"))
}

impl Config {
    /// A config from upstream-shaped JSON (see the module documentation). A value that is not
    /// an object is a config with nothing in it (upstream's `mergeConfig` spreads it). `Err`
    /// for a RegExp or a constructor as a value (see the module's divergences).
    pub fn from_json(json: &Json) -> R<Config> {
        let mut config = Config::default();
        let Json::Object(_) = json else {
            return Ok(config);
        };
        for (key, v) in entries(json) {
            let v = v.as_ref();
            match key.as_str() {
                "nodes" => config.nodes = schemas(v, "nodes")?,
                "tags" => config.tags = schemas(v, "tags")?,
                "variables" => config.variables = Some(Variables::Value(value(v, "variables")?)),
                "functions" => config.functions = functions(v)?,
                "partials" => {
                    if v.is_null() {
                        config.partials_null = true;
                    } else {
                        config.partials = Some(partials(v)?);
                    }
                }
                "validation" => config.validation = validation(v),
                _ => {}
            }
        }
        Ok(config)
    }
}

/// `{...tags, ...config.tags}`'s part: each entry's schema.
fn schemas(v: &Json, at: &str) -> R<IndexMap<String, Option<Schema>>> {
    let mut out = IndexMap::new();
    for (name, s) in entries(v) {
        out.insert(name.clone(), schema(&s, &format!("{at}.{name}"))?);
    }
    Ok(out)
}

/// A schema: `None` when falsy; an empty schema for a truthy value that is not an object
/// (every field upstream reads is `undefined`).
fn schema(v: &Json, at: &str) -> R<Option<Schema>> {
    if !truthy(v) {
        return Ok(None);
    }
    let Json::Object(_) = v else {
        return Ok(Some(Schema::default()));
    };
    let mut s = Schema::default();
    for (key, v) in entries(v) {
        let v = v.as_ref();
        let here = format!("{at}.{key}");
        match key.as_str() {
            // `!schema.render`, then `new Tag(schema.render, ...)`.
            "render" => s.render = truthy(v).then(|| value(v, &here)).transpose()?,
            // `schema.children && ... !schema.children.includes(type)`.
            "children" => s.children = truthy(v).then(|| value(v, &here)).transpose()?,
            // `{...globalAttributes, ...schema.attributes}`.
            "attributes" => {
                let mut out = IndexMap::new();
                for (name, a) in entries(v) {
                    out.insert(name.clone(), attribute(&a, &format!("{here}.{name}"))?);
                }
                s.attributes = (!out.is_empty()).then_some(out);
            }
            // `if (schema.slots) ... Object.entries(schema.slots)`.
            "slots" => {
                s.slots = if truthy(v) {
                    let mut out = IndexMap::new();
                    for (name, slot) in entries(v) {
                        out.insert(name.clone(), schema_slot(&slot, &format!("{here}.{name}"))?);
                    }
                    Some(out)
                } else {
                    None
                }
            }
            "selfClosing" => s.self_closing = truthy(v),
            // `schema.inline != undefined && node.inline !== schema.inline`.
            "inline" => s.inline = (!v.is_null()).then(|| value(v, &here)).transpose()?,
            "description" => s.description = v.as_str().map(str::to_string),
            // `schema.transform instanceof Function`: JSON holds no function.
            "transform" => {}
            // `if (schema.validate) ... schema.validate(node, config)`.
            "validate" => {
                s.validate = truthy(v).then(|| -> ValidateFn {
                    Arc::new(|_, _| Err(not_a_function("schema.validate")))
                });
            }
            _ => {}
        }
    }
    Ok(Some(s))
}

/// An attribute's (or a function parameter's) schema.
fn attribute(v: &Json, at: &str) -> R<SchemaAttribute> {
    let mut a = SchemaAttribute {
        kind: kind(v),
        ..SchemaAttribute::default()
    };
    if a.kind != SchemaKind::Object {
        return Ok(a);
    }
    for (key, v) in entries(v) {
        let v = v.as_ref();
        let here = format!("{at}.{key}");
        match key.as_str() {
            "type" => {
                a.attr_type = if truthy(v) {
                    Some(type_spec(v, &here)?)
                } else {
                    None
                }
            }
            "render" => a.render = Some(value(v, &here)?),
            "default" => a.default = Some(value(v, &here)?),
            "required" => a.required = truthy(v),
            "matches" => a.matches = Some(matches(v, &here)?),
            // `errorLevel || 'error'`: any truthy value is the level.
            "errorLevel" => a.error_level = truthy(v).then(|| ValidationLevel::from_json(v)),
            "description" => a.description = v.as_str().map(str::to_string),
            // `typeof attrib.validate === 'function'`: JSON holds no function.
            "validate" => {}
            _ => {}
        }
    }
    Ok(a)
}

fn schema_slot(v: &Json, at: &str) -> R<SchemaSlot> {
    let mut slot = SchemaSlot {
        kind: kind(v),
        ..SchemaSlot::default()
    };
    if slot.kind != SchemaKind::Object {
        return Ok(slot);
    }
    for (key, v) in entries(v) {
        let v = v.as_ref();
        match key.as_str() {
            "render" => slot.render = Some(value(v, at)?),
            "required" => slot.required = truthy(v),
            _ => {}
        }
    }
    Ok(slot)
}

/// `{"$type": name}`: `TYPES[name]`, a constructor or one of upstream's classes, or a name
/// every object inherits (`TYPES.constructor` is `Object`; `toString` a function that is no
/// constructor; `__proto__` `Object.prototype`, no function at all); [`TypeSpec::Falsy`] for
/// a name `TYPES` does not have (`undefined`).
fn constructor(o: &serde_json::Map<String, Json>, _at: &str) -> R<Option<TypeSpec>> {
    let Some(Json::String(name)) = o.get("$type") else {
        return Ok(None);
    };
    Ok(Some(type_from_value(&crate::next::realm::harness_type(
        name,
    ))))
}

/// The type a revived value is.
fn type_from_value(v: &Value) -> TypeSpec {
    use crate::next::realm::Builtin;
    let Value::Builtin(b) = v else {
        return TypeSpec::Falsy;
    };
    match *b {
        Builtin::STRING => TypeSpec::Builtin(BuiltinType::String),
        Builtin::NUMBER => TypeSpec::Builtin(BuiltinType::Number),
        Builtin::BOOLEAN => TypeSpec::Builtin(BuiltinType::Boolean),
        Builtin::OBJECT => TypeSpec::Builtin(BuiltinType::Object),
        Builtin::ARRAY => TypeSpec::Builtin(BuiltinType::Array),
        Builtin::CLASS => TypeSpec::Markdoc(MarkdocType::Class),
        Builtin::ID => TypeSpec::Markdoc(MarkdocType::Id),
        Builtin::CONDITIONAL_ATTRIBUTE_TYPE => {
            TypeSpec::Markdoc(MarkdocType::ConditionalAttributeType)
        }
        Builtin::PARTIAL_FILE => TypeSpec::Markdoc(MarkdocType::PartialFile),
        other => TypeSpec::Realm(other),
    }
}

/// A type: a name, a constructor or class, a list of them, or any other truthy value (no
/// value's `constructor` is it).
fn type_spec(v: &Json, at: &str) -> R<TypeSpec> {
    if !truthy(v) {
        return Ok(TypeSpec::Falsy);
    }
    match v {
        Json::String(name) => Ok(TypeSpec::Name(name.clone())),
        Json::Array(items) => Ok(TypeSpec::Union(
            items
                .iter()
                .map(|i| type_spec(i, at))
                .collect::<R<Vec<_>>>()?,
        )),
        Json::Object(o) => match constructor(o, at)? {
            Some(t) => Ok(t),
            // A RegExp (`$regex`) as a type is an object like any other here: no
            // `constructor` is it, and it has no `name`.
            None => Ok(TypeSpec::Value(Value::from_json(v))),
        },
        _ => Ok(TypeSpec::Value(Value::from_json(v))),
    }
}

fn matches(v: &Json, at: &str) -> R<Matches> {
    Ok(match v {
        Json::Array(items) => {
            Matches::Array(items.iter().map(|i| value(i, at)).collect::<R<Vec<_>>>()?)
        }
        Json::Object(o) if o.get("$regex").is_some_and(Json::is_string) => {
            let source = o["$regex"].as_str().unwrap_or_default();
            let flags = match o.get("flags") {
                None | Some(Json::Null) => Cow::Borrowed(""),
                Some(Json::String(f)) => Cow::Borrowed(f.as_str()),
                // `value.flags || ''`, then `new RegExp(source, flags)` makes it a string.
                Some(other) if !truthy(other) => Cow::Borrowed(""),
                Some(other) => {
                    Cow::Owned(Value::from_json(other).to_js_string()?.into_string_lossy())
                }
            };
            Matches::RegExp(Arc::new(JsRegExp::new(source, &flags)?))
        }
        // A function as `matches` is called with the config (`matches = matches(config)`).
        Json::Object(o) if o.get("$type").is_some_and(Json::is_string) => match value(v, at)? {
            Value::Builtin(f) if f.is_callable() => Matches::Call(f),
            _ => Matches::Other,
        },
        // Neither an array nor a RegExp: nothing to match.
        _ => {
            check_plain(v, at)?;
            Matches::Other
        }
    })
}

/// `{...functions, ...config.functions}`'s part.
fn functions(v: &Json) -> R<IndexMap<String, Option<ConfigFunction>>> {
    let mut out = IndexMap::new();
    for (name, f) in entries(v) {
        out.insert(name.clone(), function(&f, &format!("functions.{name}"))?);
    }
    Ok(out)
}

/// A function: `None` when falsy; one with nothing upstream reads for a truthy value that is
/// not an object.
fn function(v: &Json, at: &str) -> R<Option<ConfigFunction>> {
    if !truthy(v) {
        return Ok(None);
    }
    let Json::Object(_) = v else {
        return Ok(Some(ConfigFunction::default()));
    };
    let mut f = ConfigFunction::default();
    for (key, v) in entries(v) {
        let v = v.as_ref();
        let here = format!("{at}.{key}");
        match key.as_str() {
            "returns" => f.returns = returns(v, &here)?,
            // `if (schema.parameters) ... schema.parameters?.[key]`.
            "parameters" => {
                f.parameters = if truthy(v) {
                    let mut out = IndexMap::new();
                    for (name, p) in entries(v) {
                        out.insert(name.clone(), attribute(&p, &format!("{here}.{name}"))?);
                    }
                    Some(out)
                } else {
                    None
                }
            }
            "description" => f.description = v.as_str().map(str::to_string),
            // `fn.transform?.(parameters, config)`: `null` returns `undefined`, anything else
            // that is no function throws.
            "transform" => {
                f.transform = (!v.is_null()).then(|| -> FunctionTransformFn {
                    Arc::new(|_, _| {
                        Err(MarkdocError::TransformError(
                            "TypeError: fn.transform is not a function".into(),
                        ))
                    })
                });
            }
            // `if (schema.validate) errors.push(...schema.validate(fn, config))`.
            "validate" => {
                f.validate = truthy(v).then(|| -> FunctionValidateFn {
                    Arc::new(|_, _| Err(not_a_function("schema.validate")))
                });
            }
            _ => {}
        }
    }
    Ok(Some(f))
}

/// `returns` as a list of types (`!schema?.returns` is no list); each compared with `===` to
/// an attribute's type: a name by its text, a constructor by identity, any other value as
/// JavaScript compares it (an object to nothing).
fn returns(v: &Json, at: &str) -> R<Option<Vec<TypeSpec>>> {
    if !truthy(v) {
        return Ok(None);
    }
    let one = |v: &Json| -> R<TypeSpec> {
        Ok(match v {
            Json::String(name) => TypeSpec::Name(name.clone()),
            Json::Object(o) => match constructor(o, at)? {
                Some(t) => t,
                None => TypeSpec::Value(Value::from_json(v)),
            },
            other => TypeSpec::Value(Value::from_json(other)),
        })
    };
    Ok(Some(match v {
        Json::Array(items) => items.iter().map(one).collect::<R<Vec<_>>>()?,
        // `{"$type": name}` naming nothing is `undefined`: no `returns`.
        Json::Object(o) if matches!(constructor(o, at)?, Some(TypeSpec::Falsy)) => {
            return Ok(None);
        }
        _ => vec![one(v)?],
    }))
}

/// `partials` (not `null`), looked up as `partials[file]`: an object's own properties; an
/// array's items and a string's characters, and their `length` (what `partials['length']`
/// finds: a number, which upstream's transform then cannot `resolve`); nothing for a number
/// or a boolean.
fn partials(v: &Json) -> R<IndexMap<String, Partial>> {
    let mut out = IndexMap::new();
    for (name, p) in entries(v) {
        out.insert(name.clone(), Partial::Value(value(&p, "partials")?));
    }
    let length = match v {
        Json::Array(items) => Some(items.len()),
        Json::String(s) => Some(s.encode_utf16().count()),
        _ => None,
    };
    if let Some(length) = length {
        out.insert(
            "length".into(),
            Partial::Value(Value::Number(length as f64)),
        );
    }
    Ok(out)
}

/// `{...config.validation, parents}`: its `validateFunctions` read for truthiness.
fn validation(v: &Json) -> Validation {
    let mut out = Validation::default();
    for (key, v) in entries(v) {
        match key.as_str() {
            "validateFunctions" => out.validate_functions = truthy(&v),
            "environment" => out.environment = v.as_str().map(str::to_string),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_an_upstream_config() {
        let c = Config::from_json(&json!({
            "tags": {"foo": {"render": "div", "attributes": {
                "b": {"type": "String", "render": "data-b", "matches": {"$regex": "^a", "flags": "i"}},
                "1": {"type": [{"$type": "Number"}, "Foo"], "default": 2, "required": true}
            }, "selfClosing": 1, "inline": false}, "off": null},
            "variables": {"x": [1, {"y": null}]},
            "functions": {"f": {"returns": "String", "parameters": {"0": {"required": true}}}},
            "partials": {"p.md": "text"},
            "validation": {"validateFunctions": true}
        }))
        .unwrap();
        let foo = c.tags["foo"].as_ref().unwrap();
        assert_eq!(foo.render.as_ref().and_then(Value::as_str), Some("div"));
        let attrs = foo.attributes.as_ref().unwrap();
        // JS key order: "1" before "b".
        assert_eq!(attrs.keys().collect::<Vec<_>>(), ["1", "b"]);
        assert!(matches!(attrs["b"].matches, Some(Matches::RegExp(_))));
        assert!(matches!(attrs["1"].attr_type, Some(TypeSpec::Union(ref t)) if t.len() == 2));
        assert!(foo.self_closing);
        assert!(matches!(foo.inline, Some(Value::Boolean(false))));
        assert!(c.tags["off"].is_none());
        assert!(c.validation.validate_functions);
        assert!(matches!(
            c.partials.as_ref().unwrap()["p.md"],
            Partial::Value(_)
        ));
    }

    #[test]
    fn every_shape_reads_as_upstream_reads_it() {
        // `__proto__` is an own key (`JSON.parse`), copied by upstream's spreads.
        let c = Config::from_json(&json!({"tags": {"__proto__": {"render": "x"}}})).unwrap();
        assert!(c.tags["__proto__"].is_some());
        // A render that is not a string, children that are a string, a non-boolean inline.
        let c = Config::from_json(&json!({"tags": {"a": {
            "render": 5, "children": "inline", "inline": 1,
            "attributes": {"x": null, "y": 0, "z": "s"}, "slots": "ab"
        }}}))
        .unwrap();
        let a = c.tags["a"].as_ref().unwrap();
        assert!(matches!(a.render, Some(Value::Number(n)) if n == 5.0));
        assert!(matches!(a.allows_child("line"), Some(Ok(true))));
        assert!(matches!(a.inline, Some(Value::Number(_))));
        let attrs = a.attributes.as_ref().unwrap();
        assert_eq!(attrs["x"].kind, SchemaKind::Null);
        assert_eq!(attrs["y"].kind, SchemaKind::Falsy);
        assert_eq!(attrs["z"].kind, SchemaKind::Truthy);
        assert_eq!(
            a.slots.as_ref().unwrap().keys().collect::<Vec<_>>(),
            ["0", "1"]
        );
        // Any error level; a type that is an object; a `validate` that is no function.
        let c = Config::from_json(&json!({"tags": {"a": {
            "attributes": {"x": {"errorLevel": "fatal", "type": {"name": "T"}}},
            "validate": true
        }}}))
        .unwrap();
        let a = c.tags["a"].as_ref().unwrap();
        let x = &a.attributes.as_ref().unwrap()["x"];
        assert_eq!(x.error_level, Some(ValidationLevel::Other(json!("fatal"))));
        assert!(matches!(x.attr_type, Some(TypeSpec::Value(_))));
        assert!(a.validate.is_some());
        // A config that is not an object is an empty one.
        assert!(Config::from_json(&json!([1])).unwrap().tags.is_empty());
    }

    #[test]
    fn types_are_revived_wherever_they_are() {
        use crate::next::realm::Builtin;
        let c = Config::from_json(&json!({
            "variables": {"t": {"$type": "String"}, "u": {"$type": "toString"},
                          "p": {"$type": "__proto__"}, "n": {"$type": "nothing"},
                          "l": [{"$type": "Class"}]},
            "tags": {"x": {"attributes": {
                "a": {"type": {"$type": "constructor"}, "matches": {"$type": "Number"}},
                "b": {"type": {"$type": "valueOf"}, "default": {"$type": "Array"}},
                "c": {"type": {"$type": "nothing"}}
            }}},
            "functions": {"f": {"returns": {"$type": "nothing"}}}
        }))
        .unwrap();
        let Some(Variables::Value(Value::Object(v))) = &c.variables else {
            panic!("variables")
        };
        // node: TYPES.String, TYPES.toString, TYPES.__proto__, TYPES.nothing.
        assert!(matches!(v.get("t"), Some(Value::Builtin(Builtin::STRING))));
        assert!(matches!(
            v.get("u"),
            Some(Value::Builtin(Builtin::OBJECT_PROTOTYPE_TO_STRING))
        ));
        assert!(matches!(
            v.get("p"),
            Some(Value::Builtin(Builtin::OBJECT_PROTOTYPE))
        ));
        assert!(matches!(v.get("n"), Some(Value::Undefined)));
        let attrs = c.tags["x"].as_ref().unwrap().attributes.as_ref().unwrap();
        // TYPES.constructor is Object.
        assert!(matches!(
            attrs["a"].attr_type,
            Some(TypeSpec::Builtin(BuiltinType::Object))
        ));
        assert!(matches!(
            attrs["a"].matches,
            Some(Matches::Call(Builtin::NUMBER))
        ));
        assert!(matches!(
            attrs["b"].attr_type,
            Some(TypeSpec::Realm(Builtin::OBJECT_PROTOTYPE_VALUE_OF))
        ));
        assert!(matches!(
            attrs["b"].default,
            Some(Value::Builtin(Builtin::ARRAY))
        ));
        assert!(matches!(attrs["c"].attr_type, Some(TypeSpec::Falsy)));
        // `returns: undefined`: no returns.
        assert!(c.functions["f"].as_ref().unwrap().returns.is_none());
    }

    #[test]
    fn unmodeled_shapes_are_errors() {
        // A RegExp as a value: the one shape left that the port does not model.
        for bad in [
            json!({"variables": {"r": {"$regex": "a"}}}),
            json!({"tags": {"a": {"attributes": {"x": {"default": {"$regex": "a"}}}}}}),
        ] {
            assert!(Config::from_json(&bad).is_err(), "{bad}");
        }
    }
}
