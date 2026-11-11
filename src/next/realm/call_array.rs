//! `Array` and `Array.prototype`'s functions, called (see [`super::call`]).

// Each arm of a dispatch `match` runs in a closure of its own, called where it is made, so
// that its locals have a frame of their own: a debug build gives a function one frame for
// every arm's locals, which a nested conversion keeps live at each level of a recursion.
#![allow(clippy::redundant_closure_call)]

use super::call::no_such_function;
use super::call::{
    Called, Text, argument as arg, callback_error, done, done_bool, done_num, done_str, has_prop,
    indices, integer, iterate, key_of, len_of, length, object_value, opaque, range,
    require_coercible, same_zero, set_on, string, type_err,
};
use super::{Builtin, is_object};
use crate::next::schema::value::{JsString, Value, get_property};

type R<T> = Result<T, String>;

/// The greatest length `ArrayCreate` accepts.
const MAX_ARRAY_LENGTH: f64 = 4294967295.0;

/// The most items a call here makes into one array: node makes up to [`MAX_ARRAY_LENGTH`]
/// (a `toReversed` of `{length: 1e9}` fills a billion `undefined`s, which takes it minutes and
/// gigabytes); past this the port throws `RangeError: Invalid array length` instead.
pub const MAX_DENSE: f64 = 16_777_216.0;

/// `ArraySpeciesCreate(O, n)` for an `O` that is not an array (every `this` a call can get
/// but `Array.prototype`, whose `constructor` is `Array`): `ArrayCreate(n)`.
fn species_create(n: f64) -> R<Vec<Value>> {
    if n > MAX_ARRAY_LENGTH {
        return Err(range("Invalid array length"));
    }
    Ok(Vec::new())
}

/// The relative index `ToIntegerOrInfinity(v)` clamped into `0..=len` (the start and end
/// arguments of `slice`, `fill`, `copyWithin`, `splice`, ...).
fn relative(v: &Value, len: f64, default: f64) -> R<f64> {
    if v.is_undefined() {
        return Ok(default);
    }
    let rel = integer(v)?;
    Ok(if rel == f64::NEG_INFINITY {
        0.0
    } else if rel < 0.0 {
        (len + rel).max(0.0)
    } else {
        rel.min(len)
    })
}

/// The items of `O` from `0..len` that are present, with their indices (holes skipped).
fn present(o: &Value, len: f64) -> R<Vec<(f64, Value)>> {
    let mut out = Vec::new();
    for k in indices(o, len) {
        out.push((k, get_property(o, &key_of(k))?));
    }
    Ok(out)
}

/// `Get(O, k)` for every `k` in `0..len` (holes as `undefined`), for a `len` that is small
/// enough to list; past [`MAX_ARRAY_LENGTH`] items no array can be made anyway.
fn dense(o: &Value, len: f64) -> R<Vec<Value>> {
    if len > MAX_ARRAY_LENGTH.min(MAX_DENSE) {
        return Err(range("Invalid array length"));
    }
    let mut out = vec![Value::Undefined; len as usize];
    for (k, v) in present(o, len)? {
        out[k as usize] = v;
    }
    Ok(out)
}

/// The callback argument, checked.
fn callback(args: &[Value]) -> R<Value> {
    let f = arg(args, 0);
    if !super::call::is_callable(&f) {
        return Err(callback_error(&f));
    }
    Ok(f)
}

/// `Array.prototype.join` of an array-like: each position's text (`""` for a hole,
/// `undefined` and `null`), a separator before each but the first.
fn join(o: &Value, separator: &Value) -> R<JsString> {
    let len = len_of(o)?;
    let sep = if separator.is_undefined() {
        JsString::from(",")
    } else {
        string(separator)?
    };
    let mut out = Text::new();
    // Positions before `next` are written.
    let mut next = 0.0;
    let separators = |out: &mut Text, upto: f64, next: f64| -> R<()> {
        // A separator before every position in `next..upto` but position 0.
        let count = upto - next - if next == 0.0 && upto > 0.0 { 1.0 } else { 0.0 };
        if count <= 0.0 {
            return Ok(());
        }
        out.reserve(count * sep.len_utf16() as f64)?;
        let mut i = 0.0;
        while i < count {
            out.push(&sep)?;
            i += 1.0;
        }
        Ok(())
    };
    for (k, v) in present(o, len)? {
        // The holes before `k`, then the separator before `k`.
        separators(&mut out, k + 1.0, next)?;
        if !v.is_nullish() {
            out.push(&string(&v)?)?;
        }
        next = k + 1.0;
    }
    separators(&mut out, len, next)?;
    Ok(out.finish())
}

pub(super) fn array_function(f: Builtin, this: &Value, args: &[Value]) -> R<Called> {
    let a0 = arg(args, 0);
    match f {
        Builtin::ARRAY => (|| -> R<Called> {
            if args.len() == 1 {
                if let Value::Number(n) = a0 {
                    let len = n.trunc().rem_euclid(4294967296.0);
                    if len != n || len > MAX_DENSE {
                        return Err(range("Invalid array length"));
                    }
                    return done(Value::array(vec![Value::Undefined; len as usize]));
                }
                return done(Value::array(vec![a0]));
            }
            done(Value::array(args.to_vec()))
        })(),
        Builtin::ARRAY_IS_ARRAY => {
            (|| -> R<Called> { done_bool(crate::next::schema::value::is_array(&a0)) })()
        }
        Builtin::ARRAY_OF => (|| -> R<Called> { done(Value::array(args.to_vec())) })(),
        Builtin::ARRAY_FROM => (|| -> R<Called> {
            let mapfn = arg(args, 1);
            if !mapfn.is_undefined() && !super::call::is_callable(&mapfn) {
                return Err(callback_error(&mapfn));
            }
            if a0.is_nullish() {
                return Err(type_err(format!(
                    "{} is not iterable (cannot read property Symbol(Symbol.iterator))",
                    super::call::describe_primitive(&a0)
                )));
            }
            let items = match &a0 {
                Value::String(_) | Value::Array(_) => iterate(&a0, "not iterable")?,
                Value::Builtin(Builtin::ARRAY_PROTOTYPE) => Vec::new(),
                _ => {
                    let o = object_value(&a0)?;
                    let len = len_of(&o)?;
                    dense(&o, len)?
                }
            };
            if !mapfn.is_undefined() {
                let this_arg = arg(args, 2);
                let mut out = Vec::new();
                for (i, v) in items.into_iter().enumerate() {
                    out.push(called_value(super::call::call_value(
                        &mapfn,
                        &this_arg,
                        &[v, Value::Number(i as f64)],
                    )?));
                }
                return done(Value::array(out));
            }
            done(Value::array(items))
        })(),
        // A promise: see the module documentation of `call` ("The event loop").
        Builtin::ARRAY_FROM_ASYNC => (|| -> R<Called> { Ok(opaque()) })(),
        _ => prototype_function(f, this, args),
    }
}

/// A call's value as an item of an array (a wrapper is its object; JSON reads it as its
/// primitive either way).
fn called_value(c: Called) -> Value {
    match c {
        Called::Value(v) | Called::Wrapper(v) => v,
    }
}

fn prototype_function(f: Builtin, this: &Value, args: &[Value]) -> R<Called> {
    let name = f
        .name()
        .strip_prefix("Array.prototype.")
        .unwrap_or(f.name());
    require_coercible(this, &format!("Array.prototype.{name}"))?;
    let o = object_value(this)?;
    let a0 = arg(args, 0);
    match f {
        Builtin::ARRAY_PROTOTYPE_AT => (|| -> R<Called> {
            let len = len_of(&o)?;
            let rel = integer(&a0)?;
            let k = if rel >= 0.0 { rel } else { len + rel };
            if k < 0.0 || k >= len {
                return done(Value::Undefined);
            }
            done(get_property(&o, &key_of(k))?)
        })(),
        Builtin::ARRAY_PROTOTYPE_CONCAT => (|| -> R<Called> {
            let mut out = Vec::new();
            for e in std::iter::once(o.clone()).chain(args.iter().cloned()) {
                if crate::next::schema::value::is_array(&e) {
                    let len = len_of(&e)?;
                    out.extend(dense(&e, len)?);
                } else {
                    out.push(e);
                }
            }
            done(Value::array(out))
        })(),
        Builtin::ARRAY_PROTOTYPE_COPY_WITHIN => (|| -> R<Called> {
            let len = len_of(&o)?;
            let to = relative(&a0, len, 0.0)?;
            let from = relative(&arg(args, 1), len, 0.0)?;
            let fin = relative(&arg(args, 2), len, len)?;
            let count = (fin - from).min(len - to);
            if count <= 0.0 {
                return done(o);
            }
            let values = dense(&o, len)?;
            let mut sets = Vec::new();
            let (from, to, count) = (from as usize, to as usize, count as usize);
            let order: Vec<usize> = if from < to && to < from + count {
                (0..count).rev().collect()
            } else {
                (0..count).collect()
            };
            let mut current = values.clone();
            for i in order {
                let src = from + i;
                let dst = to + i;
                let present = super::call::has_prop(&o, &key_of(src as f64))?;
                let v = current[src].clone();
                current[dst] = v.clone();
                sets.push((key_of(dst as f64), if present { Some(v) } else { None }));
            }
            done(set_on(&o, &sets))
        })(),
        Builtin::ARRAY_PROTOTYPE_FILL => (|| -> R<Called> {
            let len = len_of(&o)?;
            let start = relative(&arg(args, 1), len, 0.0)?;
            let end = relative(&arg(args, 2), len, len)?;
            let mut sets = Vec::new();
            let mut k = start;
            while k < end {
                sets.push((key_of(k), Some(a0.clone())));
                k += 1.0;
            }
            done(set_on(&o, &sets))
        })(),
        Builtin::ARRAY_PROTOTYPE_FIND
        | Builtin::ARRAY_PROTOTYPE_FIND_INDEX
        | Builtin::ARRAY_PROTOTYPE_FIND_LAST
        | Builtin::ARRAY_PROTOTYPE_FIND_LAST_INDEX
        | Builtin::ARRAY_PROTOTYPE_FOR_EACH
        | Builtin::ARRAY_PROTOTYPE_FILTER
        | Builtin::ARRAY_PROTOTYPE_FLAT_MAP
        | Builtin::ARRAY_PROTOTYPE_MAP
        | Builtin::ARRAY_PROTOTYPE_EVERY
        | Builtin::ARRAY_PROTOTYPE_SOME => {
            let len = len_of(&o)?;
            let cb = callback(args)?;
            let this_arg = arg(args, 1);
            iterate_with(f, &o, len, &cb, &this_arg)
        }
        Builtin::ARRAY_PROTOTYPE_REDUCE | Builtin::ARRAY_PROTOTYPE_REDUCE_RIGHT => {
            (|| -> R<Called> {
                let len = len_of(&o)?;
                let cb = callback(args)?;
                let mut items = present(&o, len)?;
                if f == Builtin::ARRAY_PROTOTYPE_REDUCE_RIGHT {
                    items.reverse();
                }
                let mut items = items.into_iter();
                let mut acc = if args.len() >= 2 {
                    arg(args, 1)
                } else {
                    match items.next() {
                        Some((_, v)) => v,
                        None => {
                            return Err(type_err("Reduce of empty array with no initial value"));
                        }
                    }
                };
                for (k, v) in items {
                    acc = called_value(super::call::call_value(
                        &cb,
                        &Value::Undefined,
                        &[acc, v, Value::Number(k), o.clone()],
                    )?);
                }
                done(acc)
            })()
        }
        Builtin::ARRAY_PROTOTYPE_LAST_INDEX_OF => (|| -> R<Called> {
            let len = len_of(&o)?;
            if len == 0.0 {
                return done_num(-1.0);
            }
            let from = if args.len() > 1 {
                integer(&arg(args, 1))?
            } else {
                len - 1.0
            };
            if from == f64::NEG_INFINITY {
                return done_num(-1.0);
            }
            let start = if from >= 0.0 {
                from.min(len - 1.0)
            } else {
                len + from
            };
            for (k, v) in present(&o, start + 1.0)?.into_iter().rev() {
                if crate::next::schema::value::strict_equals(&v, &a0) {
                    return done_num(k);
                }
            }
            done_num(-1.0)
        })(),
        Builtin::ARRAY_PROTOTYPE_INDEX_OF | Builtin::ARRAY_PROTOTYPE_INCLUDES => {
            (|| -> R<Called> {
                let len = len_of(&o)?;
                let includes = f == Builtin::ARRAY_PROTOTYPE_INCLUDES;
                if len == 0.0 {
                    return if includes {
                        done_bool(false)
                    } else {
                        done_num(-1.0)
                    };
                }
                let n = integer(&arg(args, 1))?;
                if n == f64::INFINITY {
                    return if includes {
                        done_bool(false)
                    } else {
                        done_num(-1.0)
                    };
                }
                let start = if n >= 0.0 { n } else { (len + n).max(0.0) };
                if includes {
                    // Holes read as undefined.
                    if a0.is_undefined() {
                        let present_count =
                            indices(&o, len).into_iter().filter(|&k| k >= start).count() as f64;
                        if present_count < len - start {
                            return done_bool(true);
                        }
                    }
                    for (k, v) in present(&o, len)? {
                        if k >= start && same_zero(&v, &a0) {
                            return done_bool(true);
                        }
                    }
                    return done_bool(false);
                }
                for (k, v) in present(&o, len)? {
                    if k >= start && crate::next::schema::value::strict_equals(&v, &a0) {
                        return done_num(k);
                    }
                }
                done_num(-1.0)
            })()
        }
        Builtin::ARRAY_PROTOTYPE_JOIN => (|| -> R<Called> { done_str(join(&o, &a0)?) })(),
        Builtin::ARRAY_PROTOTYPE_TO_STRING => (|| -> R<Called> {
            let join_fn = get_property(&o, "join")?;
            if super::call::is_callable(&join_fn) {
                return super::call::call_value(&join_fn, &o, &[]);
            }
            done_str(super::call::object_to_string(&o))
        })(),
        Builtin::ARRAY_PROTOTYPE_TO_LOCALE_STRING => (|| -> R<Called> {
            let len = len_of(&o)?;
            let mut out = Text::new();
            let mut k = 0.0;
            let items = present(&o, len)?;
            let mut items = items.into_iter().peekable();
            while k < len {
                if k > 0.0 {
                    out.push_str(",")?;
                }
                if items.peek().is_some_and(|(i, _)| *i == k) {
                    let (_, v) = items.next().expect("peeked");
                    if !v.is_nullish() {
                        // `Invoke(item, "toLocaleString", « locales, options »)`.
                        let f = get_property(&v, "toLocaleString")?;
                        let r = super::call::call_value(&f, &v, &args[..args.len().min(2)])?;
                        out.push(&string(&called_value(r))?)?;
                    }
                } else if items.peek().is_none() {
                    // Only holes are left: the separators.
                    out.reserve(len - k - 1.0)?;
                    while k + 1.0 < len {
                        out.push_str(",")?;
                        k += 1.0;
                    }
                }
                k += 1.0;
            }
            done_str(out.finish())
        })(),
        Builtin::ARRAY_PROTOTYPE_KEYS
        | Builtin::ARRAY_PROTOTYPE_ENTRIES
        | Builtin::ARRAY_PROTOTYPE_VALUES => Ok(opaque()),
        Builtin::ARRAY_PROTOTYPE_POP => (|| -> R<Called> {
            let len = len_of(&o)?;
            if len == 0.0 {
                return done(Value::Undefined);
            }
            let index = key_of(len - 1.0);
            done(get_property(&o, &index)?)
        })(),
        Builtin::ARRAY_PROTOTYPE_PUSH | Builtin::ARRAY_PROTOTYPE_UNSHIFT => (|| -> R<Called> {
            let len = len_of(&o)?;
            let n = args.len() as f64;
            if len + n > 9007199254740991.0 {
                return Err(type_err(format!(
                    "Pushing {} elements on an array-like of length {} is disallowed, as the total surpasses 2**53-1",
                    args.len(),
                    crate::markdown_it::js::number_to_string(len)
                )));
            }
            done_num(len + n)
        })(),
        Builtin::ARRAY_PROTOTYPE_SHIFT => (|| -> R<Called> {
            let len = len_of(&o)?;
            if len == 0.0 {
                return done(Value::Undefined);
            }
            done(get_property(&o, "0")?)
        })(),
        Builtin::ARRAY_PROTOTYPE_REVERSE => (|| -> R<Called> {
            let len = len_of(&o)?;
            let items = present(&o, len)?;
            let mut sets: Vec<(String, Option<Value>)> = Vec::new();
            let present_at: std::collections::HashMap<u64, Value> =
                items.into_iter().map(|(k, v)| (k as u64, v)).collect();
            let mut lower = 0.0;
            let half = (len / 2.0).floor();
            while lower < half {
                let upper = len - lower - 1.0;
                let lv = present_at.get(&(lower as u64)).cloned();
                let uv = present_at.get(&(upper as u64)).cloned();
                if lv.is_some() || uv.is_some() {
                    sets.push((key_of(lower), uv));
                    sets.push((key_of(upper), lv));
                }
                lower += 1.0;
                if lower > 1e7 {
                    break;
                }
            }
            done(set_on(&o, &sets))
        })(),
        Builtin::ARRAY_PROTOTYPE_SLICE => (|| -> R<Called> {
            let len = len_of(&o)?;
            let k = relative(&a0, len, 0.0)?;
            let fin = relative(&arg(args, 1), len, len)?;
            let count = (fin - k).max(0.0);
            species_create(count)?;
            if count > MAX_DENSE {
                return Err(range("Invalid array length"));
            }
            let mut out = vec![Value::Undefined; count as usize];
            for (i, v) in present(&o, fin)? {
                if i >= k {
                    out[(i - k) as usize] = v;
                }
            }
            done(Value::array(out))
        })(),
        Builtin::ARRAY_PROTOTYPE_SORT | Builtin::ARRAY_PROTOTYPE_TO_SORTED => (|| -> R<Called> {
            if !a0.is_undefined() && !super::call::is_callable(&a0) {
                return Err(type_err(format!(
                    "The comparison function must be either a function or undefined: {}",
                    super::call::describe_primitive(&a0)
                )));
            }
            let len = len_of(&o)?;
            if f == Builtin::ARRAY_PROTOTYPE_TO_SORTED && len > MAX_ARRAY_LENGTH {
                return Err(range("Invalid array length"));
            }
            let items: Vec<Value> = present(&o, len)?.into_iter().map(|(_, v)| v).collect();
            let sorted = sort_values(items, &a0)?;
            if f == Builtin::ARRAY_PROTOTYPE_TO_SORTED {
                if len > MAX_DENSE {
                    return Err(range("Invalid array length"));
                }
                let mut out = sorted;
                out.resize(len as usize, Value::Undefined);
                return done(Value::array(out));
            }
            let count = sorted.len() as f64;
            let mut sets: Vec<(String, Option<Value>)> = sorted
                .into_iter()
                .enumerate()
                .map(|(i, v)| (key_of(i as f64), Some(v)))
                .collect();
            for k in indices(&o, len) {
                if k >= count {
                    sets.push((key_of(k), None));
                }
            }
            done(set_on(&o, &sets))
        })(),
        Builtin::ARRAY_PROTOTYPE_SPLICE | Builtin::ARRAY_PROTOTYPE_TO_SPLICED => {
            (|| -> R<Called> {
                let len = len_of(&o)?;
                let start = relative(&a0, len, 0.0)?;
                let (skip, insert): (f64, usize) = match args.len() {
                    0 => (0.0, 0),
                    1 => (len - start, 0),
                    n => {
                        let dc = integer(&arg(args, 1))?;
                        (dc.max(0.0).min(len - start), n - 2)
                    }
                };
                if f == Builtin::ARRAY_PROTOTYPE_TO_SPLICED {
                    let new_len = len + insert as f64 - skip;
                    if new_len > 9007199254740991.0 {
                        return Err(type_err("Invalid array length"));
                    }
                    if new_len > MAX_ARRAY_LENGTH {
                        return Err(range("Invalid array length"));
                    }
                    let values = dense(&o, len)?;
                    let mut out: Vec<Value> = values[..start as usize].to_vec();
                    out.extend(args.iter().skip(2).cloned());
                    out.extend(values[(start + skip) as usize..].iter().cloned());
                    return done(Value::array(out));
                }
                if len + insert as f64 - skip > 9007199254740991.0 {
                    return Err(type_err("Invalid array length"));
                }
                species_create(skip)?;
                if skip > MAX_DENSE {
                    return Err(range("Invalid array length"));
                }
                let mut removed = vec![Value::Undefined; skip as usize];
                for (k, v) in present(&o, start + skip)? {
                    if k >= start {
                        removed[(k - start) as usize] = v;
                    }
                }
                done(Value::array(removed))
            })()
        }
        Builtin::ARRAY_PROTOTYPE_FLAT => (|| -> R<Called> {
            let len = len_of(&o)?;
            let depth = if a0.is_undefined() {
                1.0
            } else {
                let d = integer(&a0)?;
                if d < 0.0 { 0.0 } else { d }
            };
            let mut out = Vec::new();
            flatten_into(&mut out, &o, len, depth)?;
            done(Value::array(out))
        })(),
        Builtin::ARRAY_PROTOTYPE_TO_REVERSED => (|| -> R<Called> {
            let len = len_of(&o)?;
            if len > MAX_ARRAY_LENGTH {
                return Err(range("Invalid array length"));
            }
            let mut out = dense(&o, len)?;
            out.reverse();
            done(Value::array(out))
        })(),
        Builtin::ARRAY_PROTOTYPE_WITH => (|| -> R<Called> {
            let len = len_of(&o)?;
            let rel = integer(&a0)?;
            let actual = if rel >= 0.0 { rel } else { len + rel };
            if actual >= len || actual < 0.0 {
                return Err(range(format!(
                    "Invalid index : {}",
                    super::call::describe_primitive(&a0)
                )));
            }
            if len > MAX_ARRAY_LENGTH {
                return Err(range("Invalid array length"));
            }
            let mut out = dense(&o, len)?;
            out[actual as usize] = arg(args, 1);
            done(Value::array(out))
        })(),
        other => Err(no_such_function(other)),
    }
}

/// `FlattenIntoArray(target, source, sourceLen, 0, depth)`.
fn flatten_into(out: &mut Vec<Value>, source: &Value, len: f64, depth: f64) -> R<()> {
    for (_, v) in present(source, len)? {
        if depth > 0.0 && crate::next::schema::value::is_array(&v) {
            let inner = len_of(&v)?;
            flatten_into(out, &v, inner, depth - 1.0)?;
        } else {
            out.push(v);
        }
    }
    Ok(())
}

/// The callback loops: `find`, `findIndex`, `findLast`, `findLastIndex`, `forEach`,
/// `filter`, `flatMap`, `map`, `every`, `some`.
fn iterate_with(f: Builtin, o: &Value, len: f64, cb: &Value, this_arg: &Value) -> R<Called> {
    let call = |v: &Value, k: f64| -> R<Value> {
        Ok(called_value(super::call::call_value(
            cb,
            this_arg,
            &[v.clone(), Value::Number(k), o.clone()],
        )?))
    };
    match f {
        Builtin::ARRAY_PROTOTYPE_FIND | Builtin::ARRAY_PROTOTYPE_FIND_INDEX => (|| -> R<Called> {
            let mut k = 0.0;
            while k < len {
                let v = get_property(o, &key_of(k))?;
                if call(&v, k)?.to_boolean() {
                    return if f == Builtin::ARRAY_PROTOTYPE_FIND {
                        done(v)
                    } else {
                        done_num(k)
                    };
                }
                k += 1.0;
            }
            if f == Builtin::ARRAY_PROTOTYPE_FIND {
                done(Value::Undefined)
            } else {
                done_num(-1.0)
            }
        })(),
        Builtin::ARRAY_PROTOTYPE_FIND_LAST | Builtin::ARRAY_PROTOTYPE_FIND_LAST_INDEX => {
            (|| -> R<Called> {
                let mut k = len - 1.0;
                while k >= 0.0 {
                    let v = get_property(o, &key_of(k))?;
                    if call(&v, k)?.to_boolean() {
                        return if f == Builtin::ARRAY_PROTOTYPE_FIND_LAST {
                            done(v)
                        } else {
                            done_num(k)
                        };
                    }
                    k -= 1.0;
                }
                if f == Builtin::ARRAY_PROTOTYPE_FIND_LAST {
                    done(Value::Undefined)
                } else {
                    done_num(-1.0)
                }
            })()
        }
        _ => (|| -> R<Called> {
            let mut out = Vec::new();
            for (k, v) in present(o, len)? {
                let r = call(&v, k)?;
                match f {
                    Builtin::ARRAY_PROTOTYPE_FILTER if r.to_boolean() => out.push(v),
                    Builtin::ARRAY_PROTOTYPE_MAP => out.push(r),
                    Builtin::ARRAY_PROTOTYPE_FLAT_MAP => {
                        if crate::next::schema::value::is_array(&r) {
                            let inner = len_of(&r)?;
                            flatten_into(&mut out, &r, inner, 0.0)?;
                        } else {
                            out.push(r);
                        }
                    }
                    Builtin::ARRAY_PROTOTYPE_EVERY if !r.to_boolean() => {
                        return done_bool(false);
                    }
                    Builtin::ARRAY_PROTOTYPE_SOME if r.to_boolean() => return done_bool(true),
                    _ => {}
                }
            }
            match f {
                Builtin::ARRAY_PROTOTYPE_FOR_EACH => done(Value::Undefined),
                Builtin::ARRAY_PROTOTYPE_EVERY => done_bool(true),
                Builtin::ARRAY_PROTOTYPE_SOME => done_bool(false),
                Builtin::ARRAY_PROTOTYPE_MAP => {
                    if len > MAX_DENSE {
                        return Err(range("Invalid array length"));
                    }
                    let mut dense = vec![Value::Undefined; len as usize];
                    for ((k, _), v) in present(o, len)?.into_iter().zip(out) {
                        dense[k as usize] = v;
                    }
                    done(Value::array(dense))
                }
                _ => done(Value::array(out)),
            }
        })(),
    }
}

/// `SortIndexedProperties` with `CompareArrayElements`: `undefined` last, the rest by
/// `ToString` code units (or by the comparator).
fn sort_values(items: Vec<Value>, comparator: &Value) -> R<Vec<Value>> {
    let (defined, undefined): (Vec<Value>, Vec<Value>) =
        items.into_iter().partition(|v| !v.is_undefined());
    if !comparator.is_undefined() {
        // A call passes no comparator but a string, which the caller has refused.
        return Err(type_err(
            "The comparison function must be either a function or undefined",
        ));
    }
    let mut sorted = tim_sort_default(defined)?;
    sorted.extend(undefined);
    Ok(sorted)
}

/// `SortCompareDefault(x, y)`: `ToString(x)`, then `ToString(y)`, compared code unit by code
/// unit. V8 converts both again at every comparison (no caching: a conversion that throws or
/// recurses does so at the first comparison that reaches it).
fn compare_default(x: &Value, y: &Value) -> R<std::cmp::Ordering> {
    let xs = string(x)?;
    let ys = string(y)?;
    Ok(xs.to_utf16().cmp(&ys.to_utf16()))
}

/// V8's `ComputeMinRunLength(n)`.
fn min_run_length(mut n: usize) -> usize {
    let mut r = 0;
    while n >= 64 {
        r |= n & 1;
        n >>= 1;
    }
    n + r
}

/// V8's TimSort over the defined items with the default comparator, comparing in V8's order:
/// each run made by `CountAndMakeRun` (a descending run reversed) and extended to the minimum
/// run length by `BinaryInsertionSort` (below 64 items that is the whole sort), then the runs
/// merged, stably (V8 merges with galloping, which compares in another order; that only shows
/// in which of several items whose conversion throws throws first, past 64 items).
fn tim_sort_default(mut work: Vec<Value>) -> R<Vec<Value>> {
    let n = work.len();
    if n < 2 {
        return Ok(work);
    }
    let min_run = min_run_length(n);
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut low = 0;
    let mut remaining = n;
    while remaining != 0 {
        let mut run = count_and_make_run(&mut work, low, low + remaining)?;
        if run < min_run {
            let forced = min_run.min(remaining);
            binary_insertion_sort(&mut work, low, low + run, low + forced)?;
            run = forced;
        }
        runs.push((low, run));
        low += run;
        remaining -= run;
    }
    while runs.len() > 1 {
        let mut merged = Vec::with_capacity(runs.len().div_ceil(2));
        for pair in runs.chunks(2) {
            if let [(a, la), (b, lb)] = *pair {
                let left: Vec<Value> = work[a..a + la].to_vec();
                let right: Vec<Value> = work[b..b + lb].to_vec();
                let (mut i, mut j, mut k) = (0, 0, a);
                while i < left.len() && j < right.len() {
                    if compare_default(&right[j], &left[i])? == std::cmp::Ordering::Less {
                        work[k] = right[j].clone();
                        j += 1;
                    } else {
                        work[k] = left[i].clone();
                        i += 1;
                    }
                    k += 1;
                }
                for v in left[i..].iter().chain(&right[j..]) {
                    work[k] = v.clone();
                    k += 1;
                }
                merged.push((a, la + lb));
            } else {
                merged.push(pair[0]);
            }
        }
        runs = merged;
    }
    Ok(work)
}

/// V8's `CountAndMakeRun(low, high)`: the length of the run starting at `low`, a strictly
/// descending one reversed in place.
fn count_and_make_run(work: &mut [Value], low_arg: usize, high: usize) -> R<usize> {
    let low = low_arg + 1;
    if low == high {
        return Ok(1);
    }
    let mut run = 2;
    let descending = compare_default(&work[low], &work[low - 1])? == std::cmp::Ordering::Less;
    let mut previous = low;
    for idx in low + 1..high {
        let order = compare_default(&work[idx], &work[previous])?;
        if descending {
            if order != std::cmp::Ordering::Less {
                break;
            }
        } else if order == std::cmp::Ordering::Less {
            break;
        }
        previous = idx;
        run += 1;
    }
    if descending {
        work[low_arg..low_arg + run].reverse();
    }
    Ok(run)
}

/// V8's `BinaryInsertionSort(low, start, high)`: `work[low..start]` is sorted; each item from
/// `start` up to `high` is inserted after the items not greater than it.
fn binary_insertion_sort(work: &mut [Value], low: usize, start: usize, high: usize) -> R<()> {
    let mut start = if low == start { start + 1 } else { start };
    while start < high {
        let (mut left, mut right) = (low, start);
        let pivot = work[start].clone();
        while left < right {
            let mid = left + ((right - left) >> 1);
            if compare_default(&pivot, &work[mid])? == std::cmp::Ordering::Less {
                right = mid;
            } else {
                left = mid + 1;
            }
        }
        work[left..=start].rotate_right(1);
        start += 1;
    }
    Ok(())
}

#[allow(dead_code)]
fn is_obj(v: &Value) -> bool {
    is_object(v)
}

#[allow(dead_code)]
fn len_value(v: &Value) -> R<f64> {
    length(v)
}

#[allow(dead_code)]
fn has(v: &Value, k: &str) -> R<bool> {
    has_prop(v, k)
}
