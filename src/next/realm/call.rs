//! Calling a built-in function: what each function of the table does when upstream calls it.
//!
//! Upstream calls a built-in where it converts a value: `String(value)` (and a template
//! literal, and a property key) calls the `toString` the value's chain finds, then its
//! `valueOf` (`OrdinaryToPrimitive`), with no arguments; `JSON.stringify` calls a `toJSON`
//! with the member's key; a `matches` that is a function is called with the config and
//! `undefined` as `this`. For a built-in object these are its own prototypes' methods
//! (`String(Array.prototype)` is `Array.prototype.toString` with `Array.prototype` as `this`);
//! an object a document makes can hold any function of the table under those keys
//! (`{toString: $s.trim}`), and upstream then calls that function with the object as `this`.
//! [`call`] is such a call, for every function of the table, as V8 runs it: the same steps in
//! the same order (a conversion that throws throws first), the same result, the same error
//! with V8's message. `tests/next/realm/conversions.jsonl` pins every function both ways
//! against node, and `findings/conformance/work/realm/convfuzz.cjs` fuzzes them.
//!
//! What a call makes that is not a value a document can hold (an iterator, a promise, a
//! RegExp, a function compiled by `Function`, a getter found by `__lookupGetter__`, an object
//! made by `Object.create`) only ever meets the callers above: `ToPrimitive`, for which it is
//! an object (not a primitive), and `JSON.stringify`, which writes such an object as `{}` and
//! a function as nothing. Each is returned as a value that does the same there: an empty
//! plain object, or `Function.prototype` for a function. A primitive wrapped in a new object
//! (`Object('a')`) is [`Called::Wrapper`]: an object to `ToPrimitive`, its primitive to
//! `JSON.stringify`.
//!
//! V8 names the call site in its error for calling what is not a function: see [`Site`] (the
//! table's [`super::FRAMELESS`] says which built-ins keep `JSON.stringify`'s).
//!
//! # Divergences from upstream
//!
//! - **Mutating `this`.** Array.prototype's `copyWithin`, `fill`, `pop`, `push`, `reverse`,
//!   `shift`, `sort`, `splice` and `unshift`, and `Node.prototype.push` (on `this.children`),
//!   set properties of the object they are called on. A call returns what V8's returns (for
//!   those that return `this`, the object as the call left it), but the object a document
//!   made is a value here, not a mutable JavaScript object: a later conversion of the same
//!   object does not see what the call set. Only an object holding one of these functions
//!   under `toString`, `valueOf` or `toJSON` is such a `this`, and only an object converted
//!   twice shows it: the harness's `JSON.stringify` of the render tree and then its HTML
//!   (`{% x a={toString: $a.pop, toJSON: $s.at, 0: true, length: 2} /%}`: `a="true"` in node,
//!   whose `JSON.stringify` popped the `undefined` at index 1, `a="undefined"` here), a
//!   partial's `variables` read twice. Values with interior mutability (the object model's)
//!   would close it.
//! - **`Function(body)` and `GeneratorFunction(body)`** compile JavaScript: V8 throws a
//!   `SyntaxError` with its parser's message for a body that does not parse (`Function('a
//!   b')`: `Unexpected identifier 'b'`), and returns a function otherwise. This crate is not
//!   a JavaScript engine: every body is taken to parse. Only `JSON.stringify` calling a
//!   `toJSON` that is one of them passes a body: the member's key (`{"a b": {toJSON:
//!   $x.constructor.constructor}}`).
//! - **Locale data.** `String.prototype.localeCompare` compares with ICU's root collation,
//!   `normalize` with Unicode's normalization data, `toLocaleUpperCase`/`toLocaleLowerCase`
//!   and `Number.prototype.toLocaleString` with a locale's data (a `toJSON` passes the key as
//!   the locale; `Array.prototype.toLocaleString` passes it on to the items). Here
//!   `localeCompare` is exact for ASCII text (ICU's root weights of ASCII, generated from
//!   node) and compares code units beyond, `normalize` changes no ASCII string (any other
//!   string is returned as it is), a locale that is not a well-formed language tag throws
//!   V8's `RangeError` and any other formats as node's default locale (en-US) does (`"de"`
//!   gives `1,234.5` where node gives `1.234,5`), and the locale-specific case mappings are
//!   `tr`, `az` and `lt`'s. The ICU4X crates (`icu_collator`, `icu_normalizer`, `icu_locale`)
//!   would close this, with their data in the crate.
//! - **V8's number-format cache.** Once any `Number.prototype.toLocaleString()` without a
//!   locale has run in a node process, V8 reuses that formatter for a locale of `""` (the
//!   cache's key for "no locale") instead of throwing `RangeError: Incorrect locale
//!   information provided` as a fresh process does; the port always throws.
//! - **The event loop.** `Array.fromAsync` returns a promise that settles later; when it
//!   rejects (it does for `Array.fromAsync()`), node reports the unhandled rejection after the
//!   transform has returned and ends the process. Here it is an object, and nothing follows.
//! - **Sorting more than 64 items** (`Array.prototype.sort` as a `toString`, the default
//!   comparison): V8 merges the runs of its TimSort with galloping, here a plain merge, so
//!   when several items' conversions throw, another may throw first. Below 64 items the
//!   comparisons are V8's, in V8's order.
//! - **Depth.** A call that converts `this` again (`{toString: $s.trim}`: `trim` converts its
//!   `this` to a string, which calls `trim`) recurses until V8's stack runs out (about 10,000
//!   frames); here [`MAX_CALL_DEPTH`] nested calls, or fewer where the transform around them
//!   already holds most of the stack it shares with them
//!   (`transformer::call_fits_stack`), throw the same `RangeError`. A finite nesting deeper
//!   than that (200 objects inside each other, each converting the next) throws here and not
//!   in node.
//! - **Upstream's AST classes' methods** run as the dist does for the objects they can be
//!   called on (a document's plain objects, `{toString: $n.transform, children: []}`), with
//!   the config a call gives them (none, or a string); where one would call back a method of
//!   its own `this` other than the built-ins' (`children.flatMap` replaced by another
//!   function, an `Iterator` whose `next` never ends), it is an error naming it. Only an AST
//!   instance a value holds (the object model's) reaches them.

// Each arm of a dispatch `match` runs in a closure of its own, called where it is made, so
// that its locals have a frame of their own: a debug build gives a function one frame for
// every arm's locals, which a nested conversion keeps live at each level of a recursion.
#![allow(clippy::redundant_closure_call)]

use super::{Builtin, Data, Slot, Wrapped, is_object, prototype_of};
use crate::markdown_it::js;
use crate::next::schema::value::{
    self, JsString, JsStringBuilder, RANGE_ERROR, Value, get_property, has_own_property,
};
use crate::types::JsObject;
use std::cell::Cell;

/// What a call returns.
#[derive(Debug, Clone)]
pub enum Called {
    /// A value: a primitive, or an object (see the module's documentation for the objects a
    /// call makes that no document can hold).
    Value(Value),
    /// A new object wrapping this primitive (`Object('a')`, `Object.assign('a')`): an object
    /// to `ToPrimitive`; `JSON.stringify` writes the primitive.
    Wrapper(Value),
}

impl Called {
    /// The value, a wrapper as the object it is to `ToPrimitive`'s check (never a primitive).
    pub fn is_primitive(&self) -> bool {
        match self {
            Called::Value(v) => !is_object(v),
            Called::Wrapper(_) => false,
        }
    }

    /// The primitive, when the result is one.
    pub fn primitive(self) -> Option<Value> {
        match self {
            Called::Value(v) if !is_object(&v) => Some(v),
            _ => None,
        }
    }
}

/// Where a call of a built-in was made from, which V8 names in the error for calling
/// something that is not a function (`Function.prototype.call` or `apply` with an object as
/// `this`). V8 prints the source text of the call the topmost JavaScript frame is making when
/// that frame is script code: so when `JSON.stringify` (C++, no JavaScript frame of its own)
/// calls a `toJSON` that is `call` or `apply` (assembly, no frame either), the frame is
/// upstream's, at its `JSON.stringify(...)` call, and the error is `TypeError:
/// JSON.stringify is not a function`. Anything else runs inside a built-in that has a frame
/// without a script (`String()`, `slice`, `join`, `toLocaleString`, a template literal's
/// conversion is no call at all), and V8 prints the value's type instead (`TypeError: object
/// is not a function`, `string "tr" is not a function`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
    /// Inside a built-in, or a conversion.
    Convert,
    /// `JSON.stringify` calling a `toJSON` directly.
    Json,
}

thread_local! {
    static SITE: Cell<Site> = const { Cell::new(Site::Convert) };
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Runs `f`, whose calls of built-ins are made from `site` (see [`Site`]).
pub fn at_site<T>(site: Site, f: impl FnOnce() -> T) -> T {
    let before = SITE.replace(site);
    let out = f();
    SITE.set(before);
    out
}

/// How many calls of built-ins may run inside each other on a thread; one more throws
/// `RangeError: Maximum call stack size exceeded`, as V8 does where its stack runs out.
pub const MAX_CALL_DEPTH: usize = 200;

struct Depth;

#[cfg(test)]
thread_local! {
    /// The stack's address at the first call and the lowest one seen, and the deepest level
    /// reached: what a level of calls costs on the stack (`tests::the_stack_a_call_costs`).
    pub(super) static STACK_PROBE: Cell<(usize, usize, usize)> = const { Cell::new((0, usize::MAX, 0)) };
}

impl Depth {
    fn enter() -> Result<Depth, String> {
        let depth = DEPTH.get();
        #[cfg(test)]
        {
            let here = &depth as *const usize as usize;
            STACK_PROBE.with(|p| {
                let (top, low, deepest) = p.get();
                let top = if depth == 0 { here } else { top };
                p.set((top, low.min(here), deepest.max(depth + 1)));
            });
        }
        if depth >= MAX_CALL_DEPTH || !crate::next::transformer::call_fits_stack() {
            return Err(RANGE_ERROR.into());
        }
        DEPTH.set(depth + 1);
        Ok(Depth)
    }
}

impl Drop for Depth {
    fn drop(&mut self) {
        DEPTH.set(DEPTH.get() - 1);
    }
}

type R<T> = Result<T, String>;

pub(super) fn type_error(message: impl AsRef<str>) -> String {
    format!("TypeError: {}", message.as_ref())
}

pub(super) fn range_error(message: impl AsRef<str>) -> String {
    format!("RangeError: {}", message.as_ref())
}

/// V8's longest string (`String.prototype.length`'s limit on 64-bit hosts).
pub const MAX_STRING_LENGTH: usize = (1 << 29) - 24;

fn invalid_string_length() -> String {
    range_error("Invalid string length")
}

/// The argument at `i`, `undefined` past the end.
pub(super) fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Undefined)
}

/// `typeof value`.
pub fn type_of(value: &Value) -> &'static str {
    match value {
        Value::Undefined => "undefined",
        Value::Null => "object",
        Value::Boolean(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) | Value::Object(_) | Value::Tag(_) => "object",
        Value::Builtin(b) if b.is_callable() => "function",
        Value::Builtin(_) => "object",
    }
}

/// `IsCallable(value)`.
pub fn is_callable(value: &Value) -> bool {
    matches!(value, Value::Builtin(b) if b.is_callable())
}

/// V8's text for a value in a "... is not a function" error where no call site is found:
/// its type, then a primitive's value (`string "a"`, `object null`, `number 1`).
pub fn default_call_site(value: &Value) -> String {
    let mut out = String::from(type_of(value));
    match value {
        Value::String(s) => {
            out.push_str(" \"");
            out.push_str(&s.to_string_lossy());
            out.push('"');
        }
        Value::Null => out.push_str(" null"),
        Value::Boolean(b) => out.push_str(if *b { " true" } else { " false" }),
        Value::Number(n) => {
            out.push(' ');
            out.push_str(&js::number_to_string(*n));
        }
        _ => {}
    }
    out
}

/// The error V8 throws for calling `value`, which is not a function, from built-in code
/// (see [`Site`]).
fn not_a_function(value: &Value) -> String {
    not_a_function_at(Site::Convert, value)
}

/// [`not_a_function`] for a call made from `site`.
fn not_a_function_at(site: Site, value: &Value) -> String {
    match site {
        Site::Json => type_error("JSON.stringify is not a function"),
        Site::Convert => type_error(format!("{} is not a function", default_call_site(value))),
    }
}

/// The error for a callback that is not a function (V8 names the value itself).
pub(super) fn callback_not_a_function(value: &Value) -> String {
    type_error(format!("{} is not a function", default_call_site(value)))
}

/// A primitive as V8's messages print it (`NoSideEffectsToString`: strings unquoted).
pub fn describe_primitive(value: &Value) -> String {
    match value {
        Value::Undefined => "undefined".into(),
        Value::Null => "null".into(),
        Value::Boolean(b) => b.to_string(),
        Value::Number(n) => js::number_to_string(*n),
        Value::String(s) => s.to_string_lossy().into_owned(),
        _ => no_side_effects_to_string(value),
    }
}

/// `[[Get]]` of a data property only (V8's `GetDataProperty`: an accessor reads as
/// `undefined`, no getter runs), through the chain.
fn data_property(value: &Value, key: &str) -> Value {
    match value {
        Value::Object(o) => {
            if let Some(v) = o.get(key) {
                return v.clone();
            }
        }
        Value::Tag(_) | Value::Array(_) | Value::String(_) => {
            if let Ok(true) = has_own_property(value, key) {
                return get_property(value, key).unwrap_or(Value::Undefined);
            }
        }
        Value::Builtin(b) => {
            if let Some(p) = b.own(key) {
                return match &p.slot {
                    Slot::Data(d) => d.to_value(),
                    Slot::Accessor { .. } => Value::Undefined,
                };
            }
        }
        _ => {}
    }
    let mut proto = match prototype_of(value) {
        Value::Builtin(b) => Some(b),
        _ => None,
    };
    while let Some(b) = proto {
        if let Some(p) = b.own(key) {
            return match &p.slot {
                Slot::Data(d) => d.to_value(),
                Slot::Accessor { .. } => Value::Undefined,
            };
        }
        proto = b.proto();
    }
    Value::Undefined
}

/// V8's `Object::NoSideEffectsToString` for an object, as its error messages print one: a
/// function's source (shortened past 128 characters), `#<Name>` for an object whose
/// `toString` is `Object.prototype.toString` and whose `constructor` is a named function,
/// else `[object Class]`.
pub fn no_side_effects_to_string(value: &Value) -> String {
    if let Value::Builtin(b) = value
        && let Some(source) = b.source()
    {
        let units: Vec<u16> = source.encode_utf16().collect();
        if units.len() > 128 {
            let mut out = String::from_utf16_lossy(&units[..111]);
            out.push_str("...<omitted>...");
            out.push_str(&String::from_utf16_lossy(&units[units.len() - 2..]));
            return out;
        }
        return source.to_string();
    }
    if !is_object(value) {
        return describe_primitive(value);
    }
    if let Value::Builtin(Builtin::OBJECT_PROTOTYPE_TO_STRING) = data_property(value, "toString")
        && let Value::Builtin(ctor) = data_property(value, "constructor")
        && ctor.is_callable()
    {
        let name = match ctor.own("name").map(|p| &p.slot) {
            Some(Slot::Data(Data::String(name))) => *name,
            _ => "",
        };
        if !name.is_empty() {
            return format!("#<{name}>");
        }
    }
    format!("[object {}]", class_name(value))
}

/// V8's class name of an object (its `[[Class]]`-like name: `Object`, `Array`, `Function`,
/// `String`, ...).
fn class_name(value: &Value) -> &'static str {
    match value {
        Value::Array(_) => "Array",
        Value::Builtin(b) => b.data().class,
        _ => "Object",
    }
}

// ---------------------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------------------

/// `ToString(value)`.
pub(super) fn to_string(value: &Value) -> R<JsString> {
    value.to_js_string()
}

/// `ToNumber(value)`.
pub(super) fn to_number(value: &Value) -> R<f64> {
    value::to_number(value)
}

/// `ToIntegerOrInfinity(value)`.
pub(super) fn to_integer_or_infinity(value: &Value) -> R<f64> {
    let n = to_number(value)?;
    Ok(if n.is_nan() || n == 0.0 {
        0.0
    } else if n.is_infinite() {
        n
    } else {
        n.trunc()
    })
}

/// `ToLength(value)`.
pub(super) fn to_length(value: &Value) -> R<f64> {
    let n = to_integer_or_infinity(value)?;
    Ok(if n <= 0.0 {
        0.0
    } else {
        n.min(9007199254740991.0)
    })
}

/// `ToUint32(value)`.
pub(super) fn to_uint32(value: &Value) -> R<u32> {
    let n = to_number(value)?;
    if !n.is_finite() || n == 0.0 {
        return Ok(0);
    }
    Ok(n.trunc().rem_euclid(4294967296.0) as u32)
}

/// `ToUint16(value)`.
pub(super) fn to_uint16(value: &Value) -> R<u16> {
    let n = to_number(value)?;
    if !n.is_finite() || n == 0.0 {
        return Ok(0);
    }
    Ok(n.trunc().rem_euclid(65536.0) as u16)
}

/// `ToPropertyKey(value)` as a key of this crate's objects (a lone surrogate is U+FFFD: keys
/// are Rust strings).
pub(super) fn to_key(value: &Value) -> R<String> {
    Ok(to_string(value)?.to_string_lossy().into_owned())
}

/// The text of an index `k` (`ToString(𝔽(k))`).
pub(super) fn index_key(k: f64) -> String {
    js::number_to_string(k)
}

/// `RequireObjectCoercible(this)`, with V8's message for `name`.
pub(super) fn coercible(this: &Value, name: &str) -> R<()> {
    if this.is_nullish() {
        return Err(type_error(format!("{name} called on null or undefined")));
    }
    Ok(())
}

/// `LengthOfArrayLike(O)` of `ToObject(this)`.
pub(super) fn length_of(o: &Value) -> R<f64> {
    to_length(&get_property(o, "length")?)
}

/// `HasProperty(O, key)`: own, or on the prototype chain (a document's objects, then the
/// realm's).
pub(super) fn has_property(o: &Value, key: &str) -> R<bool> {
    let mut at = o.clone();
    loop {
        if has_own_property(&at, key)? {
            return Ok(true);
        }
        match value::prototype_of(&at) {
            Some(value::Proto::Value(p)) => at = p,
            Some(value::Proto::Builtin(b)) => return Ok(b.has_property(key)),
            Some(value::Proto::Null) | None => return Ok(false),
        }
    }
}

/// The array-index keys an object has (own or inherited) below `len`, ascending: what a
/// loop over `0..len` that skips absent indices visits.
pub(super) fn present_indices(o: &Value, len: f64) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    let mut at = Some(o.clone());
    // The object, then each document object on its chain (no prototype of the table holds an
    // index property).
    while let Some(v) = at.take() {
        match &v {
            Value::Object(obj) => out.extend(
                obj.keys()
                    .filter_map(|k| crate::types::array_index(k))
                    .map(f64::from)
                    .filter(|&i| i < len),
            ),
            Value::Array(items) => out.extend(
                items
                    .present()
                    .map(|(i, _)| f64::from(i))
                    .take_while(|&i| i < len),
            ),
            Value::String(s) => {
                out.extend((0..s.len_utf16()).map(|i| i as f64).filter(|&i| i < len))
            }
            _ => {}
        }
        if let Some(value::Proto::Value(p)) = value::prototype_of(&v) {
            at = Some(p);
        }
    }
    out.sort_by(f64::total_cmp);
    out.dedup();
    out
}

/// A copy of a plain object with `key` set (what a call that sets a property of `this` leaves:
/// see the module's documentation).
pub(super) fn with_set(o: &Value, sets: &[(String, Option<Value>)]) -> Value {
    match o {
        Value::Object(obj) => {
            let (mut copy, proto) = (**obj).clone().into_parts();
            for (k, v) in sets {
                match v {
                    Some(v) => {
                        copy.insert(k.clone(), v.clone());
                    }
                    None => {
                        copy.remove(k);
                    }
                }
            }
            // The copy keeps the object's prototype.
            Value::object_with_proto(copy, proto)
        }
        Value::Array(items) => {
            // Holes stay holes; a deleted element becomes one.
            let mut length = items.len() as u64;
            let mut present: std::collections::BTreeMap<u32, Value> =
                items.present().map(|(i, v)| (i, v.clone())).collect();
            for (k, v) in sets {
                if k == "length" {
                    if let Some(Value::Number(n)) = v {
                        length = *n as u64;
                        present.retain(|&i, _| u64::from(i) < length);
                    }
                } else if let Some(i) = crate::types::array_index(k) {
                    match v {
                        Some(v) => {
                            present.insert(i, v.clone());
                            length = length.max(u64::from(i) + 1);
                        }
                        None => {
                            present.remove(&i);
                        }
                    }
                }
            }
            let named: indexmap::IndexMap<String, Value> =
                items.named().map(|(k, v)| (k.clone(), v.clone())).collect();
            Value::sparse_array(length.min(u64::from(u32::MAX)) as u32, present, named)
        }
        other => other.clone(),
    }
}

/// A new plain object from entries.
fn object_of(entries: Vec<(&str, Value)>) -> Value {
    let mut o = JsObject::new();
    for (k, v) in entries {
        o.insert(k, v);
    }
    Value::object(o)
}

/// What `JSON.stringify` and `ToPrimitive` see of an object a call made that no document can
/// hold (an iterator, a promise, a RegExp, `Object.create`'s object): `{}`.
pub(super) fn opaque_object() -> Called {
    Called::Value(Value::empty_object())
}

/// What they see of a function a call made (`Function()`'s, a getter): a function.
pub(super) fn opaque_function() -> Called {
    Called::Value(Value::Builtin(Builtin::FUNCTION_PROTOTYPE))
}

pub(super) fn ok(v: Value) -> R<Called> {
    Ok(Called::Value(v))
}

pub(super) fn ok_str(s: impl Into<JsString>) -> R<Called> {
    ok(Value::String(s.into()))
}

pub(super) fn ok_num(n: f64) -> R<Called> {
    ok(Value::Number(n))
}

pub(super) fn ok_bool(b: bool) -> R<Called> {
    ok(Value::Boolean(b))
}

/// `SameValue(a, b)`.
fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            (x.is_nan() && y.is_nan()) || (x == y && x.is_sign_negative() == y.is_sign_negative())
        }
        _ => value::strict_equals(a, b),
    }
}

/// `SameValueZero(a, b)`.
pub(super) fn same_value_zero(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => (x.is_nan() && y.is_nan()) || x == y,
        _ => value::strict_equals(a, b),
    }
}

// ---------------------------------------------------------------------------------------
// The call
// ---------------------------------------------------------------------------------------

/// `Call(f, this, args)` for a function `f` of the table: what V8 does (see the module's
/// documentation). `Err` is what it throws, as `Name: message`. A `f` that is not callable
/// throws V8's "is not a function".
pub fn call(f: Builtin, this: &Value, args: &[Value]) -> Result<Called, String> {
    // The site this call is made from. What a function with a frame of its own calls is
    // called from inside that frame; a frameless one's calls are made from its caller's site.
    let site = SITE.get();
    if !f.is_frameless() {
        SITE.set(Site::Convert);
    }
    // Built-ins call one another (a conversion's `toString`, `join` of nested arrays): each
    // call on a stack with room for it ([`crate::depth::grow!`]).
    let out = crate::depth::grow!(call_from(site, f, this, args));
    SITE.set(site);
    out
}

fn call_from(site: Site, f: Builtin, this: &Value, args: &[Value]) -> Result<Called, String> {
    if !f.is_callable() {
        return Err(not_a_function(&Value::Builtin(f)));
    }
    let _depth = Depth::enter()?;
    if f.is_class() {
        let name = match f.own("name").map(|p| &p.slot) {
            Some(Slot::Data(Data::String(name))) => *name,
            _ => "",
        };
        return Err(type_error(format!(
            "Class constructor {name} cannot be invoked without 'new'"
        )));
    }
    match f {
        // Function.prototype itself is a function that returns undefined.
        Builtin::FUNCTION_PROTOTYPE => (|| -> R<Called> { ok(Value::Undefined) })(),
        // `Invoke(this, "toString")`, which can call this again (`{toString:
        // $o.toLocaleString}` recurses until the stack runs out): in this small frame.
        Builtin::OBJECT_PROTOTYPE_TO_LOCALE_STRING => (|| -> R<Called> {
            coercible(this, "Object.prototype.toLocaleString")?;
            let to_string = get_property(this, "toString")?;
            call_value(&to_string, this, &[])
        })(),
        b if is_object_function(b) => object_function(b, this, args),
        b if is_function_function(b) => function_function(site, b, this, args),
        b if is_array_function(b) => super::call_array::array_function(b, this, args),
        b if is_string_function(b) => super::call_string::string_function(b, this, args),
        b if is_number_function(b) => number_function(b, this, args),
        b if is_boolean_function(b) => boolean_function(b, this, args),
        b if is_regexp_function(b) => super::call_string::regexp_function(b, this, args),
        b if is_markdoc_function(b) => markdoc_function(b, this, args),
        b if name_starts(b, &["Node", "Variable", "AstFunction"]) => {
            super::call_ast::ast_function(b, this, args)
        }
        b if name_starts(b, &["GeneratorFunction", "Generator", "Iterator"]) => {
            super::call_ast::iterator_function(b, this, args)
        }
        other => Err(no_such_function(other)),
    }
}

/// The error for a function no family of calls has: never made, as every callable object of
/// the table belongs to one (`tests::every_function_called_as_upstream_calls_it` calls each).
pub(super) fn no_such_function(f: Builtin) -> String {
    format!("TypeError: {} has no implementation here", f.name())
}

/// Calls `value` when it is callable, else V8's error for calling it.
pub(super) fn call_value(value: &Value, this: &Value, args: &[Value]) -> R<Called> {
    match value {
        Value::Builtin(f) if f.is_callable() => call(*f, this, args),
        other => Err(not_a_function(other)),
    }
}

fn name_starts(b: Builtin, prefixes: &[&str]) -> bool {
    let name = b.name();
    prefixes.iter().any(|p| {
        name == *p
            || name
                .strip_prefix(p)
                .is_some_and(|rest| rest.starts_with('.'))
    })
}

fn is_object_function(b: Builtin) -> bool {
    name_starts(b, &["Object"])
}
fn is_function_function(b: Builtin) -> bool {
    name_starts(b, &["Function"])
}
fn is_array_function(b: Builtin) -> bool {
    name_starts(b, &["Array"])
}
fn is_string_function(b: Builtin) -> bool {
    name_starts(b, &["String"])
}
fn is_number_function(b: Builtin) -> bool {
    name_starts(b, &["Number"])
}
fn is_boolean_function(b: Builtin) -> bool {
    name_starts(b, &["Boolean"])
}
fn is_regexp_function(b: Builtin) -> bool {
    name_starts(b, &["RegExp"])
}
fn is_markdoc_function(b: Builtin) -> bool {
    name_starts(
        b,
        &[
            "Tag",
            "Class",
            "Id",
            "ConditionalAttributeType",
            "PartialFile",
        ],
    )
}

// ---------------------------------------------------------------------------------------
// Object
// ---------------------------------------------------------------------------------------

/// `ToObject(value)`: an object as it is, a primitive in a new wrapper; `TypeError` for
/// `undefined` and `null`.
fn to_object(value: &Value) -> R<Called> {
    if value.is_nullish() {
        return Err(type_error("Cannot convert undefined or null to object"));
    }
    Ok(if is_object(value) {
        Called::Value(value.clone())
    } else {
        Called::Wrapper(value.clone())
    })
}

/// The value `ToObject` makes, for reading: a wrapper reads as its primitive does.
pub(super) fn to_object_value(value: &Value) -> R<Value> {
    to_object(value).map(|c| match c {
        Called::Value(v) | Called::Wrapper(v) => v,
    })
}

/// An own property's descriptor: `(value or getter, is accessor, writable, enumerable,
/// configurable)`, or `None`.
struct Descriptor {
    value: Value,
    accessor: Option<(bool, bool)>,
    writable: bool,
    enumerable: bool,
    configurable: bool,
}

fn own_descriptor(o: &Value, key: &str) -> R<Option<Descriptor>> {
    Ok(match o {
        Value::Object(obj) => obj.get(key).map(|v| Descriptor {
            value: v.clone(),
            accessor: None,
            writable: true,
            enumerable: true,
            configurable: true,
        }),
        Value::Tag(_) => {
            if has_own_property(o, key)? {
                Some(Descriptor {
                    value: get_property(o, key)?,
                    accessor: None,
                    writable: true,
                    enumerable: true,
                    configurable: true,
                })
            } else {
                None
            }
        }
        Value::Array(items) => {
            if key == "length" {
                Some(Descriptor {
                    value: Value::Number(items.len() as f64),
                    accessor: None,
                    writable: true,
                    enumerable: false,
                    configurable: false,
                })
            } else {
                match crate::types::array_index(key) {
                    Some(i) => items.get(i as usize),
                    None => items.named_get(key),
                }
                .map(|v| Descriptor {
                    value: v.clone(),
                    accessor: None,
                    writable: true,
                    enumerable: true,
                    configurable: true,
                })
            }
        }
        Value::String(s) => {
            if key == "length" {
                Some(Descriptor {
                    value: Value::Number(s.len_utf16() as f64),
                    accessor: None,
                    writable: false,
                    enumerable: false,
                    configurable: false,
                })
            } else if let Some(i) =
                crate::types::array_index(key).filter(|&i| (i as usize) < s.len_utf16())
            {
                Some(Descriptor {
                    value: get_property(o, &i.to_string())?,
                    accessor: None,
                    writable: false,
                    enumerable: true,
                    configurable: false,
                })
            } else {
                None
            }
        }
        Value::Builtin(b) => b.own(key).map(|p| {
            let (value, accessor) = match &p.slot {
                Slot::Data(d) => (d.to_value(), None),
                Slot::Accessor { get, set } => (Value::Undefined, Some((get.is_some(), *set))),
            };
            Descriptor {
                value,
                accessor,
                writable: p.attrs & super::WRITABLE != 0,
                enumerable: p.attrs & super::ENUMERABLE != 0,
                configurable: p.attrs & super::CONFIGURABLE != 0,
            }
        }),
        _ => None,
    })
}

/// The own string keys, in `[[OwnPropertyKeys]]` order.
fn own_keys(o: &Value) -> Vec<String> {
    match o {
        Value::Object(obj) => obj.keys().cloned().collect(),
        Value::Tag(_) => ["$$mdtype", "name", "attributes", "children"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        Value::Array(items) => items
            .present()
            .map(|(i, _)| i.to_string())
            .chain(std::iter::once("length".to_string()))
            .chain(items.named().map(|(k, _)| k.clone()))
            .collect(),
        Value::String(s) => (0..s.len_utf16())
            .map(|i| i.to_string())
            .chain(std::iter::once("length".to_string()))
            .collect(),
        Value::Builtin(b) => b.own_keys().map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

/// `FromPropertyDescriptor(desc)`.
fn from_descriptor(d: Descriptor) -> Value {
    match d.accessor {
        None => object_of(vec![
            ("value", d.value),
            ("writable", Value::Boolean(d.writable)),
            ("enumerable", Value::Boolean(d.enumerable)),
            ("configurable", Value::Boolean(d.configurable)),
        ]),
        Some((get, set)) => {
            let f = |present: bool| {
                if present {
                    Value::Builtin(Builtin::FUNCTION_PROTOTYPE)
                } else {
                    Value::Undefined
                }
            };
            object_of(vec![
                ("get", f(get)),
                ("set", f(set)),
                ("enumerable", Value::Boolean(d.enumerable)),
                ("configurable", Value::Boolean(d.configurable)),
            ])
        }
    }
}

/// `EnumerableOwnProperties(O, kind)` as `(key, value)` pairs.
fn enumerable_own(o: &Value) -> R<Vec<(String, Value)>> {
    let mut out = Vec::new();
    for key in own_keys(o) {
        if let Some(d) = own_descriptor(o, &key)?
            && d.enumerable
        {
            let v = get_property(o, &key)?;
            out.push((key, v));
        }
    }
    Ok(out)
}

/// `Object.prototype.toString` of `this`.
pub(super) fn object_to_string(this: &Value) -> String {
    let tag = match this {
        Value::Undefined => "Undefined",
        Value::Null => "Null",
        Value::Boolean(_) => "Boolean",
        Value::Number(_) => "Number",
        Value::String(_) => "String",
        Value::Array(_) => "Array",
        Value::Object(_) | Value::Tag(_) => "Object",
        Value::Builtin(b) => b.data().class,
    };
    format!("[object {tag}]")
}

fn object_function(f: Builtin, this: &Value, args: &[Value]) -> R<Called> {
    let a0 = arg(args, 0);
    match f {
        Builtin::OBJECT => (|| -> R<Called> {
            if a0.is_nullish() {
                Ok(opaque_object())
            } else {
                to_object(&a0)
            }
        })(),
        Builtin::OBJECT_ASSIGN => (|| -> R<Called> {
            let target = to_object(&a0)?;
            for source in args.iter().skip(1) {
                if source.is_nullish() {
                    continue;
                }
                let from = to_object_value(source)?;
                if !enumerable_own(&from)?.is_empty() {
                    // Properties copied onto the target: only a primitive target is new here.
                    return Ok(match target {
                        Called::Value(t) => {
                            let sets: Vec<(String, Option<Value>)> = enumerable_own(&from)?
                                .into_iter()
                                .map(|(k, v)| (k, Some(v)))
                                .collect();
                            Called::Value(with_set(&t, &sets))
                        }
                        wrapper => wrapper,
                    });
                }
            }
            Ok(target)
        })(),
        Builtin::OBJECT_GET_OWN_PROPERTY_DESCRIPTOR => (|| -> R<Called> {
            let o = to_object_value(&a0)?;
            let key = to_key(&arg(args, 1))?;
            ok(own_descriptor(&o, &key)?
                .map(from_descriptor)
                .unwrap_or(Value::Undefined))
        })(),
        Builtin::OBJECT_GET_OWN_PROPERTY_DESCRIPTORS => (|| -> R<Called> {
            let o = to_object_value(&a0)?;
            let mut out = JsObject::new();
            for key in own_keys(&o) {
                if let Some(d) = own_descriptor(&o, &key)? {
                    out.insert(key, from_descriptor(d));
                }
            }
            ok(Value::object(out))
        })(),
        Builtin::OBJECT_GET_OWN_PROPERTY_NAMES => (|| -> R<Called> {
            let o = to_object_value(&a0)?;
            ok(Value::array(
                own_keys(&o).into_iter().map(Value::from).collect(),
            ))
        })(),
        Builtin::OBJECT_GET_OWN_PROPERTY_SYMBOLS => (|| -> R<Called> {
            to_object_value(&a0)?;
            ok(Value::array(Vec::new()))
        })(),
        Builtin::OBJECT_HAS_OWN => (|| -> R<Called> {
            let o = to_object_value(&a0)?;
            let key = to_key(&arg(args, 1))?;
            ok_bool(has_own_property(&o, &key)?)
        })(),
        Builtin::OBJECT_IS => (|| -> R<Called> { ok_bool(same_value(&a0, &arg(args, 1))) })(),
        Builtin::OBJECT_PREVENT_EXTENSIONS | Builtin::OBJECT_SEAL | Builtin::OBJECT_FREEZE => {
            (|| -> R<Called> { ok(a0) })()
        }
        Builtin::OBJECT_CREATE => (|| -> R<Called> {
            if !is_object(&a0) && !matches!(a0, Value::Null) {
                return Err(type_error(format!(
                    "Object prototype may only be an Object or null: {}",
                    describe_primitive(&a0)
                )));
            }
            let properties = arg(args, 1);
            if !properties.is_undefined() {
                let props = to_object_value(&properties)?;
                if !enumerable_own(&props)?.is_empty() {
                    return Err(type_error(
                        "Property description must be an object: not modeled",
                    ));
                }
            }
            Ok(opaque_object())
        })(),
        Builtin::OBJECT_DEFINE_PROPERTIES => (|| -> R<Called> {
            if !is_object(&a0) {
                return Err(type_error("Object.defineProperties called on non-object"));
            }
            let props = to_object_value(&arg(args, 1))?;
            if enumerable_own(&props)?.is_empty() {
                return ok(a0);
            }
            Err(type_error(
                "Object.defineProperties on an object: not modeled",
            ))
        })(),
        Builtin::OBJECT_DEFINE_PROPERTY => (|| -> R<Called> {
            if !is_object(&a0) {
                return Err(type_error("Object.defineProperty called on non-object"));
            }
            let attributes = arg(args, 2);
            if !is_object(&attributes) {
                return Err(type_error(format!(
                    "Property description must be an object: {}",
                    describe_primitive(&attributes)
                )));
            }
            Err(type_error(
                "Object.defineProperty on an object: not modeled",
            ))
        })(),
        Builtin::OBJECT_GET_PROTOTYPE_OF => (|| -> R<Called> {
            let o = to_object_value(&a0)?;
            ok(prototype_of(&o))
        })(),
        Builtin::OBJECT_SET_PROTOTYPE_OF => (|| -> R<Called> {
            if a0.is_nullish() {
                return Err(type_error(
                    "Object.setPrototypeOf called on null or undefined",
                ));
            }
            let proto = arg(args, 1);
            if !is_object(&proto) && !matches!(proto, Value::Null) {
                return Err(type_error(format!(
                    "Object prototype may only be an Object or null: {}",
                    describe_primitive(&proto)
                )));
            }
            if !is_object(&a0) {
                return ok(a0);
            }
            Err(type_error(
                "Object.setPrototypeOf on an object: not modeled",
            ))
        })(),
        Builtin::OBJECT_IS_EXTENSIBLE => (|| -> R<Called> { ok_bool(is_object(&a0)) })(),
        Builtin::OBJECT_IS_FROZEN | Builtin::OBJECT_IS_SEALED => {
            (|| -> R<Called> { ok_bool(!is_object(&a0)) })()
        }
        Builtin::OBJECT_KEYS => (|| -> R<Called> {
            let o = to_object_value(&a0)?;
            ok(Value::array(
                enumerable_own(&o)?
                    .into_iter()
                    .map(|(k, _)| Value::from(k))
                    .collect(),
            ))
        })(),
        Builtin::OBJECT_VALUES => (|| -> R<Called> {
            let o = to_object_value(&a0)?;
            ok(Value::array(
                enumerable_own(&o)?.into_iter().map(|(_, v)| v).collect(),
            ))
        })(),
        Builtin::OBJECT_ENTRIES => (|| -> R<Called> {
            let o = to_object_value(&a0)?;
            ok(Value::array(
                enumerable_own(&o)?
                    .into_iter()
                    .map(|(k, v)| Value::array(vec![Value::from(k), v]))
                    .collect(),
            ))
        })(),
        Builtin::OBJECT_FROM_ENTRIES => (|| -> R<Called> {
            let entries = iterate(&a0, "not iterable")?;
            let mut out = JsObject::new();
            for entry in entries {
                if !is_object(&entry) {
                    return Err(type_error(format!(
                        "Iterator value {} is not an entry object",
                        describe_primitive(&entry)
                    )));
                }
                let k = to_key(&get_property(&entry, "0")?)?;
                let v = get_property(&entry, "1")?;
                out.insert(k, v);
            }
            ok(Value::object(out))
        })(),
        Builtin::OBJECT_GROUP_BY => (|| -> R<Called> {
            if a0.is_nullish() {
                return Err(type_error("Object.groupBy called on null or undefined"));
            }
            let callback = arg(args, 1);
            Err(callback_not_a_function(&callback))
        })(),
        Builtin::OBJECT_PROTOTYPE_DEFINE_GETTER | Builtin::OBJECT_PROTOTYPE_DEFINE_SETTER => {
            (|| -> R<Called> {
                to_object_value(this)?;
                let which = if f == Builtin::OBJECT_PROTOTYPE_DEFINE_GETTER {
                    "__defineGetter__"
                } else {
                    "__defineSetter__"
                };
                if !is_callable(&arg(args, 1)) {
                    return Err(type_error(format!(
                        "Object.prototype.{which}: Expecting function"
                    )));
                }
                Err(type_error(format!(
                    "Object.prototype.{which} with a function: not modeled"
                )))
            })()
        }
        Builtin::OBJECT_PROTOTYPE_HAS_OWN_PROPERTY => (|| -> R<Called> {
            let key = to_key(&a0)?;
            let o = to_object_value(this)?;
            ok_bool(has_own_property(&o, &key)?)
        })(),
        Builtin::OBJECT_PROTOTYPE_LOOKUP_GETTER | Builtin::OBJECT_PROTOTYPE_LOOKUP_SETTER => {
            (|| -> R<Called> {
                let o = to_object_value(this)?;
                let key = to_key(&a0)?;
                let getter = f == Builtin::OBJECT_PROTOTYPE_LOOKUP_GETTER;
                let mut current = Some(o);
                while let Some(object) = current {
                    if let Some(d) = own_descriptor(&object, &key)? {
                        return match d.accessor {
                            Some((get, set)) if (getter && get) || (!getter && set) => {
                                Ok(opaque_function())
                            }
                            _ => ok(Value::Undefined),
                        };
                    }
                    current = match prototype_of(&object) {
                        Value::Null => None,
                        proto => Some(proto),
                    };
                }
                ok(Value::Undefined)
            })()
        }
        Builtin::OBJECT_PROTOTYPE_IS_PROTOTYPE_OF => (|| -> R<Called> {
            if !is_object(&a0) {
                return ok_bool(false);
            }
            let o = to_object_value(this)?;
            let mut proto = prototype_of(&a0);
            while !matches!(proto, Value::Null) {
                if same_value(&proto, &o) {
                    return ok_bool(true);
                }
                proto = prototype_of(&proto);
            }
            ok_bool(false)
        })(),
        Builtin::OBJECT_PROTOTYPE_PROPERTY_IS_ENUMERABLE => (|| -> R<Called> {
            let key = to_key(&a0)?;
            let o = to_object_value(this)?;
            ok_bool(own_descriptor(&o, &key)?.is_some_and(|d| d.enumerable))
        })(),
        Builtin::OBJECT_PROTOTYPE_TO_STRING => {
            (|| -> R<Called> { ok_str(object_to_string(this)) })()
        }
        Builtin::OBJECT_PROTOTYPE_VALUE_OF => (|| -> R<Called> { to_object(this) })(),
        other => Err(no_such_function(other)),
    }
}

/// The values `for (const v of value)` gives for the values a call can iterate: a string's
/// code points, an array's items; `TypeError` (`{value} is {what}`) for what has no
/// iterator.
pub(super) fn iterate(value: &Value, what: &str) -> R<Vec<Value>> {
    match value {
        Value::String(s) => {
            let units = s.to_utf16();
            let mut out = Vec::new();
            let mut i = 0;
            while i < units.len() {
                let len = if (0xD800..=0xDBFF).contains(&units[i])
                    && i + 1 < units.len()
                    && (0xDC00..=0xDFFF).contains(&units[i + 1])
                {
                    2
                } else {
                    1
                };
                out.push(Value::String(JsString::from_utf16(&units[i..i + len])));
                i += len;
            }
            Ok(out)
        }
        // The array iterator: every index below the length, a hole read as `undefined`.
        Value::Array(items) => Ok((0..items.len())
            .map(|i| items.get(i).cloned().unwrap_or(Value::Undefined))
            .collect()),
        Value::Builtin(Builtin::ARRAY_PROTOTYPE) | Value::Builtin(Builtin::STRING_PROTOTYPE) => {
            Ok(Vec::new())
        }
        other => Err(type_error(format!(
            "{} is {what}",
            describe_primitive(other)
        ))),
    }
}

// ---------------------------------------------------------------------------------------
// Function
// ---------------------------------------------------------------------------------------

fn function_function(site: Site, f: Builtin, this: &Value, args: &[Value]) -> R<Called> {
    match f {
        Builtin::FUNCTION => (|| -> R<Called> {
            // CreateDynamicFunction: every argument is converted to a string first.
            for a in args {
                to_string(a)?;
            }
            Ok(opaque_function())
        })(),
        Builtin::FUNCTION_PROTOTYPE_APPLY => (|| -> R<Called> {
            if !is_callable(this) {
                return Err(not_a_function_at(site, this));
            }
            let array = arg(args, 1);
            let list = if array.is_nullish() {
                Vec::new()
            } else if !is_object(&array) {
                return Err(type_error("CreateListFromArrayLike called on non-object"));
            } else {
                let len = length_of(&array)?;
                let mut list = Vec::new();
                let mut k = 0.0;
                while k < len {
                    list.push(get_property(&array, &index_key(k))?);
                    k += 1.0;
                }
                list
            };
            call_value(this, &arg(args, 0), &list)
        })(),
        Builtin::FUNCTION_PROTOTYPE_CALL => (|| -> R<Called> {
            if !is_callable(this) {
                return Err(not_a_function_at(site, this));
            }
            let rest: Vec<Value> = args.iter().skip(1).cloned().collect();
            call_value(this, &arg(args, 0), &rest)
        })(),
        Builtin::FUNCTION_PROTOTYPE_BIND => (|| -> R<Called> {
            if !is_callable(this) {
                return Err(type_error("Bind must be called on a function"));
            }
            Ok(opaque_function())
        })(),
        Builtin::FUNCTION_PROTOTYPE_TO_STRING => (|| -> R<Called> {
            match this {
                Value::Builtin(b) if b.is_callable() => {
                    ok_str(b.source().expect("a function has a source"))
                }
                _ => Err(type_error(
                    "Function.prototype.toString requires that 'this' be a Function",
                )),
            }
        })(),
        other => Err(no_such_function(other)),
    }
}

// ---------------------------------------------------------------------------------------
// Number and Boolean
// ---------------------------------------------------------------------------------------

/// `thisNumberValue(this)`.
fn this_number(this: &Value, method: &str) -> R<f64> {
    match this {
        Value::Number(n) => Ok(*n),
        Value::Builtin(b) => match b.data().wrapped {
            Some(Wrapped::Number(n)) => Ok(n),
            _ => Err(type_error(format!(
                "Number.prototype.{method} requires that 'this' be a Number"
            ))),
        },
        _ => Err(type_error(format!(
            "Number.prototype.{method} requires that 'this' be a Number"
        ))),
    }
}

/// `thisBooleanValue(this)`.
fn this_boolean(this: &Value, method: &str) -> R<bool> {
    match this {
        Value::Boolean(b) => Ok(*b),
        Value::Builtin(b) => match b.data().wrapped {
            Some(Wrapped::Boolean(v)) => Ok(v),
            _ => Err(type_error(format!(
                "Boolean.prototype.{method} requires that 'this' be a Boolean"
            ))),
        },
        _ => Err(type_error(format!(
            "Boolean.prototype.{method} requires that 'this' be a Boolean"
        ))),
    }
}

/// `parseFloat(string)`'s number: the longest prefix of the trimmed string that is a
/// `StrDecimalLiteral`.
fn parse_float(s: &[u16]) -> f64 {
    let s = js::trim_start(s);
    let text: String = s
        .iter()
        .take_while(|&&c| c < 0x80)
        .map(|&c| c as u8 as char)
        .collect();
    let (sign, rest) = match text.as_bytes().first() {
        Some(b'-') => (-1.0, &text[1..]),
        Some(b'+') => (1.0, &text[1..]),
        _ => (1.0, &text[..]),
    };
    if rest.starts_with("Infinity") {
        return sign * f64::INFINITY;
    }
    let b = rest.as_bytes();
    let mut i = 0;
    let int_digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    i += int_digits;
    let mut frac_digits = 0;
    if i < b.len() && b[i] == b'.' {
        frac_digits = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if int_digits > 0 || frac_digits > 0 {
            i += 1 + frac_digits;
        }
    }
    if int_digits == 0 && frac_digits == 0 {
        return f64::NAN;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exp_digits = b[j..].iter().take_while(|c| c.is_ascii_digit()).count();
        if exp_digits > 0 {
            i = j + exp_digits;
        }
    }
    let literal = &rest[..i];
    let literal = literal.strip_suffix('.').unwrap_or(literal);
    sign * literal.parse::<f64>().unwrap_or(f64::NAN)
}

/// `Number.prototype.toString(radix)` of `x` for a radix other than 10 (V8's
/// `DoubleToRadixCString`).
fn number_to_radix_string(x: f64, radix: u32) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if x == 0.0 {
        return "0".into();
    }
    const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let negative = x < 0.0;
    let value = x.abs();
    let mut integer = value.floor();
    let mut fraction = value - integer;
    // The precision V8 keeps: half the distance to the next double.
    let mut delta = 0.5 * (next_up(value) - value);
    delta = delta.max(next_up(0.0));
    let mut frac_digits = Vec::new();
    if fraction >= delta {
        loop {
            fraction *= f64::from(radix);
            delta *= f64::from(radix);
            let digit = fraction.floor() as usize;
            frac_digits.push(CHARS[digit]);
            fraction -= digit as f64;
            if (fraction > 0.5 || (fraction == 0.5 && (digit & 1) == 1)) && fraction + delta > 1.0 {
                // Round up and propagate the carry.
                loop {
                    let Some(last) = frac_digits.pop() else {
                        integer += 1.0;
                        break;
                    };
                    let d = (last as char).to_digit(36).expect("a digit") + 1;
                    if d < radix {
                        frac_digits.push(CHARS[d as usize]);
                        break;
                    }
                }
                break;
            }
            if fraction < delta {
                break;
            }
        }
    }
    let mut int_digits = Vec::new();
    // Integer part: exact digits while the integer is exact, then zeros as V8 writes them.
    let mut exponent = 0;
    while integer / f64::from(radix).powi(exponent) >= f64::from(radix) {
        exponent += 1;
    }
    let mut n = integer;
    if n < 9007199254740992.0 {
        if n == 0.0 {
            int_digits.push(b'0');
        }
        while n >= 1.0 {
            let d = (n % f64::from(radix)) as usize;
            int_digits.push(CHARS[d]);
            n = ((n - d as f64) / f64::from(radix)).floor();
        }
        int_digits.reverse();
    } else {
        let mut digits = Vec::new();
        while n >= 1.0 {
            let rem = n % f64::from(radix);
            digits.push(CHARS[rem as usize]);
            n = (n - rem) / f64::from(radix);
        }
        digits.reverse();
        int_digits = digits;
    }
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    out.push_str(std::str::from_utf8(&int_digits).expect("ASCII digits"));
    if !frac_digits.is_empty() {
        out.push('.');
        out.push_str(std::str::from_utf8(&frac_digits).expect("ASCII digits"));
    }
    out
}

fn next_up(x: f64) -> f64 {
    if x.is_nan() || x == f64::INFINITY {
        return x;
    }
    if x == 0.0 {
        return f64::from_bits(1);
    }
    let bits = x.to_bits();
    f64::from_bits(if x > 0.0 { bits + 1 } else { bits - 1 })
}

/// `x.toFixed(digits)` for `|x| < 1e21`: the exact value rounded half up at `digits` places.
fn to_fixed(x: f64, digits: usize) -> String {
    let negative = x < 0.0;
    let decimal = exact_decimal(x.abs());
    let rounded = round_decimal(&decimal, digits);
    let mut out = String::new();
    // `If x < 0, then set s to "-"`: a negative that rounds to zero is "-0.00".
    if negative {
        out.push('-');
    }
    out.push_str(&rounded);
    out
}

/// The exact decimal digits of a finite non-negative double: `(integer digits, fraction
/// digits)`.
fn exact_decimal(x: f64) -> (String, String) {
    let bits = x.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = if exponent == 0 {
        (bits & ((1 << 52) - 1)) << 1
    } else {
        (bits & ((1 << 52) - 1)) | (1 << 52)
    };
    let e = exponent - 1075;
    // value = mantissa * 2^e
    let mut digits: Vec<u32> = mantissa
        .to_string()
        .bytes()
        .map(|b| u32::from(b - b'0'))
        .collect();
    let mut frac_len = 0usize;
    if e >= 0 {
        for _ in 0..e {
            // multiply by 2
            let mut carry = 0;
            for d in digits.iter_mut().rev() {
                let v = *d * 2 + carry;
                *d = v % 10;
                carry = v / 10;
            }
            if carry > 0 {
                digits.insert(0, carry);
            }
        }
    } else {
        for _ in 0..(-e) {
            // divide by 2: multiply by 5 and shift the decimal point
            let mut carry = 0;
            for d in digits.iter_mut().rev() {
                let v = *d * 5 + carry;
                *d = v % 10;
                carry = v / 10;
            }
            if carry > 0 {
                digits.insert(0, carry);
            }
            frac_len += 1;
        }
    }
    while digits.len() <= frac_len {
        digits.insert(0, 0);
    }
    let split = digits.len() - frac_len;
    let int: String = digits[..split]
        .iter()
        .map(|d| char::from(b'0' + *d as u8))
        .collect();
    let frac: String = digits[split..]
        .iter()
        .map(|d| char::from(b'0' + *d as u8))
        .collect();
    let int = int.trim_start_matches('0').to_string();
    (if int.is_empty() { "0".into() } else { int }, frac)
}

/// `(integer, fraction)` rounded half up to `digits` fraction digits, as text.
fn round_decimal((int, frac): &(String, String), digits: usize) -> String {
    let mut all: Vec<u8> = int.bytes().chain(frac.bytes()).collect();
    let keep = int.len() + digits;
    let mut frac_padded = frac.clone();
    while frac_padded.len() < digits {
        frac_padded.push('0');
        all.push(b'0');
    }
    let round_up = all.get(keep).is_some_and(|&d| d >= b'5');
    all.truncate(keep);
    if round_up {
        let mut i = all.len();
        loop {
            if i == 0 {
                all.insert(0, b'1');
                break;
            }
            i -= 1;
            if all[i] == b'9' {
                all[i] = b'0';
            } else {
                all[i] += 1;
                break;
            }
        }
    }
    let int_len = all.len() - digits;
    let mut out = String::from_utf8(all[..int_len].to_vec()).expect("digits");
    if digits > 0 {
        out.push('.');
        out.push_str(std::str::from_utf8(&all[int_len..]).expect("digits"));
    }
    out
}

fn number_function(f: Builtin, this: &Value, args: &[Value]) -> R<Called> {
    let a0 = arg(args, 0);
    match f {
        Builtin::NUMBER => (|| -> R<Called> {
            ok_num(if args.is_empty() {
                0.0
            } else {
                to_number(&a0)?
            })
        })(),
        Builtin::NUMBER_IS_FINITE => {
            (|| -> R<Called> { ok_bool(matches!(a0, Value::Number(n) if n.is_finite())) })()
        }
        Builtin::NUMBER_IS_INTEGER => (|| -> R<Called> {
            ok_bool(matches!(a0, Value::Number(n) if n.is_finite() && n.trunc() == n))
        })(),
        Builtin::NUMBER_IS_NAN => {
            (|| -> R<Called> { ok_bool(matches!(a0, Value::Number(n) if n.is_nan())) })()
        }
        Builtin::NUMBER_IS_SAFE_INTEGER => (|| -> R<Called> {
            ok_bool(
                matches!(a0, Value::Number(n) if n.is_finite() && n.trunc() == n && n.abs() <= 9007199254740991.0),
            )
        })(),
        Builtin::NUMBER_PARSE_FLOAT => (|| -> R<Called> {
            let s = to_string(&a0)?;
            ok_num(parse_float(&s.to_utf16()))
        })(),
        Builtin::NUMBER_PARSE_INT => (|| -> R<Called> {
            let s = to_string(&a0)?;
            let radix = to_number(&arg(args, 1))?;
            let r = if radix.is_nan() || radix == 0.0 || !radix.is_finite() {
                0
            } else {
                radix.trunc().rem_euclid(4294967296.0) as i64 as i32
            };
            if r != 0 && !(2..=36).contains(&r) {
                return ok_num(f64::NAN);
            }
            ok_num(js::parse_int(&s.to_utf16(), r as u32))
        })(),
        Builtin::NUMBER_PROTOTYPE_VALUE_OF => {
            (|| -> R<Called> { ok_num(this_number(this, "valueOf")?) })()
        }
        Builtin::NUMBER_PROTOTYPE_TO_STRING => (|| -> R<Called> {
            let x = this_number(this, "toString")?;
            let radix = arg(args, 0);
            let r = if radix.is_undefined() {
                10.0
            } else {
                to_integer_or_infinity(&radix)?
            };
            if !(2.0..=36.0).contains(&r) {
                return Err(range_error("toString() radix must be between 2 and 36"));
            }
            if r == 10.0 {
                ok_str(js::number_to_string(x))
            } else {
                ok_str(number_to_radix_string(x, r as u32))
            }
        })(),
        Builtin::NUMBER_PROTOTYPE_TO_LOCALE_STRING => (|| -> R<Called> {
            let x = this_number(this, "toLocaleString")?;
            // `new Intl.NumberFormat(locales)`: a tag that is not a well-formed language tag
            // throws; any other formats as node's default locale (en-US) does here (see the
            // module's divergences: other locales' data).
            super::call_string::locale_language(&a0)?;
            ok_str(locale_number(x))
        })(),
        Builtin::NUMBER_PROTOTYPE_TO_FIXED => (|| -> R<Called> {
            let x = this_number(this, "toFixed")?;
            let f = to_integer_or_infinity(&a0)?;
            if !f.is_finite() || !(0.0..=100.0).contains(&f) {
                return Err(range_error(
                    "toFixed() digits argument must be between 0 and 100",
                ));
            }
            if !x.is_finite() {
                return ok_str(js::number_to_string(x));
            }
            if x.abs() >= 1e21 {
                return ok_str(js::number_to_string(x));
            }
            let s = to_fixed(x, f as usize);
            ok_str(if x == 0.0 {
                s.trim_start_matches('-').to_string()
            } else {
                s
            })
        })(),
        Builtin::NUMBER_PROTOTYPE_TO_EXPONENTIAL => (|| -> R<Called> {
            let x = this_number(this, "toExponential")?;
            let f = to_integer_or_infinity(&a0)?;
            if !x.is_finite() {
                return ok_str(js::number_to_string(x));
            }
            if !f.is_finite() || !(0.0..=100.0).contains(&f) {
                return Err(range_error(
                    "toExponential() argument must be between 0 and 100",
                ));
            }
            ok_str(to_exponential(
                x,
                if a0.is_undefined() {
                    None
                } else {
                    Some(f as usize)
                },
            ))
        })(),
        Builtin::NUMBER_PROTOTYPE_TO_PRECISION => (|| -> R<Called> {
            let x = this_number(this, "toPrecision")?;
            if a0.is_undefined() {
                return ok_str(js::number_to_string(x));
            }
            let p = to_integer_or_infinity(&a0)?;
            if !x.is_finite() {
                return ok_str(js::number_to_string(x));
            }
            if !p.is_finite() || !(1.0..=100.0).contains(&p) {
                return Err(range_error(
                    "toPrecision() argument must be between 1 and 100",
                ));
            }
            ok_str(to_precision(x, p as usize))
        })(),
        other => Err(no_such_function(other)),
    }
}

/// `x.toLocaleString()` in node's default locale (en-US): grouped thousands, at most three
/// fraction digits.
fn locale_number(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "∞".into() } else { "-∞".into() };
    }
    let rounded = round_decimal(&exact_decimal(x.abs()), 3);
    let (int, frac) = rounded.split_once('.').unwrap_or((&rounded, ""));
    let frac = frac.trim_end_matches('0');
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let mut out = String::new();
    if x < 0.0 && (int != "0" || !frac.is_empty()) {
        out.push('-');
    }
    out.push_str(&grouped);
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    out
}

/// The decimal digits and exponent of `x` (non-zero, finite) rounded half up to `n`
/// significant digits: `(digits, e)` with `x ≈ 0.d1d2... × 10^(e+1)`.
fn significant(x: f64, n: usize) -> (String, i32) {
    let (int, frac) = exact_decimal(x.abs());
    let all = format!("{}{}", if int == "0" { "" } else { &int }, frac);
    let leading = all.bytes().take_while(|&b| b == b'0').count();
    let e = if int == "0" {
        -(leading as i32) - 1
    } else {
        int.len() as i32 - 1
    };
    let digits: Vec<u8> = all.bytes().skip(leading).collect();
    let mut kept: Vec<u8> = digits.iter().copied().take(n).collect();
    while kept.len() < n {
        kept.push(b'0');
    }
    let mut e = e;
    if digits.get(n).is_some_and(|&d| d >= b'5') {
        let mut i = kept.len();
        loop {
            if i == 0 {
                kept.insert(0, b'1');
                kept.pop();
                e += 1;
                break;
            }
            i -= 1;
            if kept[i] == b'9' {
                kept[i] = b'0';
            } else {
                kept[i] += 1;
                break;
            }
        }
    }
    (String::from_utf8(kept).expect("digits"), e)
}

fn to_exponential(x: f64, fraction_digits: Option<usize>) -> String {
    let sign = if x < 0.0 { "-" } else { "" };
    let (digits, e) = if x == 0.0 {
        ("0".repeat(fraction_digits.unwrap_or(0) + 1), 0)
    } else {
        match fraction_digits {
            Some(f) => significant(x, f + 1),
            None => {
                let s = format!("{:e}", x.abs());
                let (m, exp) = s.split_once('e').expect("an exponent");
                (m.replace('.', ""), exp.parse().expect("an exponent"))
            }
        }
    };
    let mut out = String::from(sign);
    out.push_str(&digits[..1]);
    if digits.len() > 1 {
        out.push('.');
        out.push_str(&digits[1..]);
    }
    out.push('e');
    out.push(if e < 0 { '-' } else { '+' });
    out.push_str(&e.abs().to_string());
    out
}

fn to_precision(x: f64, p: usize) -> String {
    let sign = if x < 0.0 { "-" } else { "" };
    let (digits, e) = if x == 0.0 {
        ("0".repeat(p), 0)
    } else {
        significant(x, p)
    };
    let mut out = String::from(sign);
    if e < -6 || e >= p as i32 {
        out.push_str(&digits[..1]);
        if p > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if e < 0 { '-' } else { '+' });
        out.push_str(&e.abs().to_string());
        return out;
    }
    if e == p as i32 - 1 {
        out.push_str(&digits);
    } else if e >= 0 {
        let k = e as usize + 1;
        out.push_str(&digits[..k]);
        out.push('.');
        out.push_str(&digits[k..]);
    } else {
        out.push_str("0.");
        out.push_str(&"0".repeat((-(e + 1)) as usize));
        out.push_str(&digits);
    }
    out
}

fn boolean_function(f: Builtin, this: &Value, args: &[Value]) -> R<Called> {
    match f {
        Builtin::BOOLEAN => (|| -> R<Called> { ok_bool(arg(args, 0).to_boolean()) })(),
        Builtin::BOOLEAN_PROTOTYPE_TO_STRING => (|| -> R<Called> {
            ok_str(if this_boolean(this, "toString")? {
                "true"
            } else {
                "false"
            })
        })(),
        Builtin::BOOLEAN_PROTOTYPE_VALUE_OF => {
            (|| -> R<Called> { ok_bool(this_boolean(this, "valueOf")?) })()
        }
        other => Err(no_such_function(other)),
    }
}

// ---------------------------------------------------------------------------------------
// Markdoc's classes (their methods are upstream's dist, run here)
// ---------------------------------------------------------------------------------------

/// `{id, level, message}`.
fn validation_error(id: &str, message: String) -> Value {
    object_of(vec![
        ("id", Value::string(id)),
        ("level", Value::string("error")),
        ("message", Value::string(message)),
    ])
}

/// A template literal's `${value}`.
fn interpolate(value: &Value) -> R<String> {
    Ok(to_string(value)?.to_string_lossy().into_owned())
}

fn markdoc_function(f: Builtin, _this: &Value, args: &[Value]) -> R<Called> {
    let a0 = arg(args, 0);
    match f {
        // `!!(tag?.$$mdtype === "Tag")`.
        Builtin::TAG_IS_TAG => (|| -> R<Called> {
            if a0.is_nullish() {
                return ok_bool(false);
            }
            let t = get_property(&a0, "$$mdtype")?;
            ok_bool(matches!(t, Value::String(ref s) if s == "Tag"))
        })(),
        Builtin::CLASS_PROTOTYPE_VALIDATE => (|| -> R<Called> {
            let t = type_of(&a0);
            if t == "string" || t == "object" {
                return ok(Value::array(Vec::new()));
            }
            let key = interpolate(&arg(args, 2))?;
            ok(Value::array(vec![validation_error(
                "attribute-type-invalid",
                format!("Attribute '{key}' must be type 'string | object'"),
            )]))
        })(),
        Builtin::CLASS_PROTOTYPE_TRANSFORM => (|| -> R<Called> {
            if !a0.to_boolean() || matches!(a0, Value::String(_)) {
                return ok(a0);
            }
            let mut classes: Vec<String> = Vec::new();
            for (k, v) in enumerable_own(&to_object_value(&a0)?)? {
                if v.to_boolean() {
                    classes.push(k);
                }
            }
            ok_str(classes.join(" "))
        })(),
        Builtin::ID_PROTOTYPE_VALIDATE => (|| -> R<Called> {
            if let Value::String(s) = &a0
                && s.code_units().next().is_some_and(|c| {
                    (u16::from(b'a')..=u16::from(b'z')).contains(&c)
                        || (u16::from(b'A')..=u16::from(b'Z')).contains(&c)
                })
            {
                return ok(Value::array(Vec::new()));
            }
            ok(Value::array(vec![validation_error(
                "attribute-value-invalid",
                "The 'id' attribute must start with a letter".into(),
            )]))
        })(),
        Builtin::CONDITIONAL_ATTRIBUTE_TYPE_PROTOTYPE_VALIDATE => (|| -> R<Called> {
            let t = type_of(&a0);
            if t == "boolean" || a0.is_nullish() || t == "object" {
                return ok(Value::array(Vec::new()));
            }
            let key = interpolate(&arg(args, 2))?;
            ok(Value::array(vec![validation_error(
                "attribute-type-invalid",
                format!(
                    "Attribute '{key}' must be type 'boolean | object' (null or undefined are also allowed)"
                ),
            )]))
        })(),
        // `const { partials = {} } = config2;`, then `partials[file]`.
        Builtin::PARTIAL_FILE_PROTOTYPE_VALIDATE => (|| -> R<Called> {
            let config = arg(args, 1);
            if config.is_nullish() {
                return Err(type_error(format!(
                    "Cannot read properties of {} (reading 'partials')",
                    if config.is_undefined() {
                        "undefined"
                    } else {
                        "null"
                    }
                )));
            }
            let partials = match get_property(&config, "partials")? {
                Value::Undefined => Value::empty_object(),
                p => p,
            };
            let key = to_key(&a0)?;
            let partial = get_property(&partials, &key)?;
            if !partial.to_boolean() {
                let file = interpolate(&a0)?;
                return ok(Value::array(vec![object_of(vec![
                    ("id", Value::string("attribute-value-invalid")),
                    ("level", Value::string("error")),
                    (
                        "message",
                        Value::string(format!(
                            "Partial `{file}` not found. The 'file' attribute must be set in `config.partials`"
                        )),
                    ),
                ])]));
            }
            ok(Value::array(Vec::new()))
        })(),
        other => Err(no_such_function(other)),
    }
}

/// `JsStringBuilder` with V8's string length limit.
pub(super) struct Text {
    out: JsStringBuilder,
    len: usize,
}

impl Text {
    pub(super) fn new() -> Self {
        Text {
            out: JsStringBuilder::new(),
            len: 0,
        }
    }

    pub(super) fn push(&mut self, s: &JsString) -> R<()> {
        self.len += s.len_utf16();
        if self.len > MAX_STRING_LENGTH {
            return Err(invalid_string_length());
        }
        self.out.push_js(s);
        Ok(())
    }

    pub(super) fn push_str(&mut self, s: &str) -> R<()> {
        self.len += s.encode_utf16().count();
        if self.len > MAX_STRING_LENGTH {
            return Err(invalid_string_length());
        }
        self.out.push_str(s);
        Ok(())
    }

    /// Adds `n` more code units without writing them yet (`Err` past the limit).
    pub(super) fn reserve(&self, n: f64) -> R<()> {
        if self.len as f64 + n > MAX_STRING_LENGTH as f64 {
            return Err(invalid_string_length());
        }
        Ok(())
    }

    pub(super) fn finish(self) -> JsString {
        self.out.finish()
    }
}

// Helpers the other call modules share.
pub(super) use {
    arg as argument, callback_not_a_function as callback_error, coercible as require_coercible,
    has_property as has_prop, index_key as key_of, length_of as len_of, ok as done,
    ok_bool as done_bool, ok_num as done_num, ok_str as done_str, opaque_object as opaque,
    present_indices as indices, range_error as range, same_value_zero as same_zero,
    to_integer_or_infinity as integer, to_key as property_key, to_length as length,
    to_number as number, to_object_value as object_value, to_string as string, to_uint16 as uint16,
    to_uint32 as uint32, type_error as type_err, with_set as set_on,
};
