//! `String`, `String.prototype`, `RegExp` and `RegExp.prototype`'s functions, called (see
//! [`super::call`]).

// Each arm of a dispatch `match` runs in a closure of its own, called where it is made, so
// that its locals have a frame of their own: a debug build gives a function one frame for
// every arm's locals, which a nested conversion keeps live at each level of a recursion.
#![allow(clippy::redundant_closure_call)]

use super::call::no_such_function;
use super::call::{
    Called, Text, argument as arg, done, done_bool, done_num, done_str, integer, len_of, length,
    number, object_value, opaque, range, require_coercible, string, type_err, uint16, uint32,
};
use super::{Builtin, Wrapped, is_object};
use crate::markdown_it::js;
use crate::next::schema::JsRegExp;
use crate::next::schema::value::{JsString, Value, get_property};

type R<T> = Result<T, String>;

/// `ToString(RequireObjectCoercible(this))` for `String.prototype.{name}`.
fn this_string(this: &Value, name: &str) -> R<Vec<u16>> {
    require_coercible(this, &format!("String.prototype.{name}"))?;
    Ok(string(this)?.to_utf16().into_owned())
}

fn js(units: Vec<u16>) -> R<Called> {
    done_str(JsString::from_utf16_vec(units))
}

/// `thisStringValue(this)`.
fn this_string_value(this: &Value, method: &str) -> R<JsString> {
    match this {
        Value::String(s) => Ok(s.clone()),
        Value::Builtin(b) => match b.data().wrapped {
            Some(Wrapped::String(s)) => Ok(JsString::from(s)),
            _ => Err(type_err(format!(
                "String.prototype.{method} requires that 'this' be a String"
            ))),
        },
        _ => Err(type_err(format!(
            "String.prototype.{method} requires that 'this' be a String"
        ))),
    }
}

/// The start position `ToIntegerOrInfinity(v)` clamped to `0..=len`.
fn clamp(v: f64, len: usize) -> usize {
    if v <= 0.0 {
        0
    } else if v >= len as f64 {
        len
    } else {
        v as usize
    }
}

/// `CreateHTML(string, tag, attribute, value)`, the string already converted.
fn create_html(s: Vec<u16>, tag: &str, attribute: Option<(&str, &Value)>) -> R<Called> {
    let mut out: Vec<u16> = Vec::new();
    out.push(u16::from(b'<'));
    out.extend(tag.encode_utf16());
    if let Some((attr, value)) = attribute {
        let v = string(value)?;
        out.push(u16::from(b' '));
        out.extend(attr.encode_utf16());
        out.extend("=\"".encode_utf16());
        for unit in v.code_units() {
            if unit == u16::from(b'"') {
                out.extend("&quot;".encode_utf16());
            } else {
                out.push(unit);
            }
        }
        out.push(u16::from(b'"'));
    }
    out.push(u16::from(b'>'));
    out.extend(s);
    out.extend("</".encode_utf16());
    out.extend(tag.encode_utf16());
    out.push(u16::from(b'>'));
    if out.len() > super::call::MAX_STRING_LENGTH {
        return Err(range("Invalid string length"));
    }
    js(out)
}

/// `GetSubstitution(matched, str, position, [], undefined, replacement)` for a string search.
fn substitution(matched: &[u16], s: &[u16], position: usize, replacement: &[u16]) -> Vec<u16> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < replacement.len() {
        let c = replacement[i];
        if c == u16::from(b'$') && i + 1 < replacement.len() {
            let next = replacement[i + 1];
            match next {
                0x24 => {
                    out.push(0x24);
                    i += 2;
                    continue;
                }
                0x26 => {
                    out.extend_from_slice(matched);
                    i += 2;
                    continue;
                }
                0x60 => {
                    out.extend_from_slice(&s[..position]);
                    i += 2;
                    continue;
                }
                0x27 => {
                    let end = (position + matched.len()).min(s.len());
                    out.extend_from_slice(&s[end..]);
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Is the locale argument of `toLocale{Upper,Lower}Case` one V8 accepts, and its language
/// (`Err` is V8's `RangeError` for a list it rejects)?
pub(super) fn locale_language(locales: &Value) -> R<Option<String>> {
    if locales.is_undefined() {
        return Ok(None);
    }
    let tags: Vec<String> = match locales {
        Value::String(s) => vec![s.to_string_lossy().into_owned()],
        Value::Null => return Err(type_err("Cannot convert undefined or null to object")),
        other if is_object(other) => {
            let len = len_of(other)?;
            let mut out = Vec::new();
            let mut k = 0.0;
            while k < len {
                let v = get_property(other, &js::number_to_string(k))?;
                if !matches!(v, Value::String(_)) && !is_object(&v) {
                    return Err(type_err("Language ID should be string or object."));
                }
                out.push(string(&v)?.to_string_lossy().into_owned());
                k += 1.0;
            }
            out
        }
        _ => Vec::new(),
    };
    for tag in &tags {
        if !is_structurally_valid_language_tag(tag) {
            return Err(range("Incorrect locale information provided"));
        }
    }
    Ok(tags.first().map(|t| {
        t.split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
    }))
}

/// ECMA-402's `IsStructurallyValidLanguageTag`: a `unicode_locale_id` (UTS 35) with no
/// duplicate variants or singletons.
fn is_structurally_valid_language_tag(tag: &str) -> bool {
    let alpha = |s: &str| s.bytes().all(|b| b.is_ascii_alphabetic());
    let digit = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let alnum = |s: &str| s.bytes().all(|b| b.is_ascii_alphanumeric());
    let subtags: Vec<&str> = tag.split('-').collect();
    if subtags.iter().any(|s| s.is_empty()) {
        return false;
    }
    let mut i = 0;
    // unicode_language_subtag: alpha{2,3} | alpha{5,8}
    let lang = subtags[0];
    if !(alpha(lang) && matches!(lang.len(), 2 | 3 | 5..=8)) {
        return false;
    }
    i += 1;
    // unicode_script_subtag: alpha{4}
    if i < subtags.len() && subtags[i].len() == 4 && alpha(subtags[i]) {
        i += 1;
    }
    // unicode_region_subtag: alpha{2} | digit{3}
    if i < subtags.len()
        && ((subtags[i].len() == 2 && alpha(subtags[i]))
            || (subtags[i].len() == 3 && digit(subtags[i])))
    {
        i += 1;
    }
    // unicode_variant_subtag: alnum{5,8} | digit alnum{3}
    let mut variants: Vec<String> = Vec::new();
    while i < subtags.len() {
        let s = subtags[i];
        let variant = (matches!(s.len(), 5..=8) && alnum(s))
            || (s.len() == 4 && s.as_bytes()[0].is_ascii_digit() && alnum(s));
        if !variant {
            break;
        }
        let lower = s.to_ascii_lowercase();
        if variants.contains(&lower) {
            return false;
        }
        variants.push(lower);
        i += 1;
    }
    // extensions: singleton (alnum but not x) followed by subtags of 2-8 alnum; pu: x then 1-8.
    let mut singletons: Vec<u8> = Vec::new();
    while i < subtags.len() {
        let s = subtags[i];
        if s.len() != 1 || !alnum(s) {
            return false;
        }
        let singleton = s.as_bytes()[0].to_ascii_lowercase();
        i += 1;
        if singleton == b'x' {
            if i == subtags.len() {
                return false;
            }
            while i < subtags.len() {
                if !(matches!(subtags[i].len(), 1..=8) && alnum(subtags[i])) {
                    return false;
                }
                i += 1;
            }
            return true;
        }
        if singletons.contains(&singleton) {
            return false;
        }
        singletons.push(singleton);
        let start = i;
        while i < subtags.len()
            && subtags[i].len() >= 2
            && subtags[i].len() <= 8
            && alnum(subtags[i])
        {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    true
}

/// The language-sensitive case mappings ICU has for `tr`, `az` and `lt` (dotted and dotless
/// i); every other language maps as the root locale.
fn locale_case(s: &[u16], language: Option<&str>, upper: bool) -> Vec<u16> {
    match (language, upper) {
        (Some("tr" | "az"), true) => {
            let replaced: Vec<u16> = s
                .iter()
                .map(|&c| if c == u16::from(b'i') { 0x0130 } else { c })
                .collect();
            js::to_upper_case(&replaced)
        }
        (Some("tr" | "az"), false) => {
            let mut out = Vec::new();
            let mut i = 0;
            while i < s.len() {
                let c = s[i];
                if c == u16::from(b'I') {
                    // `I` followed by U+0307 COMBINING DOT ABOVE is `i`; alone, `ı`.
                    if s.get(i + 1) == Some(&0x0307) {
                        out.push(u16::from(b'i'));
                        i += 2;
                        continue;
                    }
                    out.push(0x0131);
                } else if c == 0x0130 {
                    out.push(u16::from(b'i'));
                } else {
                    out.extend(js::to_lower_case(&[c]));
                }
                i += 1;
            }
            out
        }
        (_, true) => js::to_upper_case(s),
        (_, false) => js::to_lower_case(s),
    }
}

/// `String.prototype.split` with a string separator.
fn split(s: &[u16], separator: &Value, limit: &Value) -> R<Called> {
    let lim = if limit.is_undefined() {
        u32::MAX
    } else {
        uint32(limit)?
    };
    let r = string(separator)?.to_utf16().into_owned();
    if lim == 0 {
        return done(Value::array(Vec::new()));
    }
    let piece = |u: &[u16]| Value::String(JsString::from_utf16(u));
    if separator.is_undefined() {
        return done(Value::array(vec![piece(s)]));
    }
    if r.is_empty() {
        let out: Vec<Value> = s
            .iter()
            .take(lim as usize)
            .map(|&u| Value::String(JsString::from_code_unit(u)))
            .collect();
        return done(Value::array(out));
    }
    if s.is_empty() {
        return done(Value::array(vec![piece(s)]));
    }
    let mut out = Vec::new();
    let mut p = 0;
    let mut q = js::index_of(s, &r, 0);
    while let Some(found) = q {
        out.push(piece(&s[p..found]));
        if out.len() as u32 == lim {
            return done(Value::array(out));
        }
        p = found + r.len();
        q = js::index_of(s, &r, p);
    }
    out.push(piece(&s[p..]));
    done(Value::array(out))
}

pub(super) fn string_function(f: Builtin, this: &Value, args: &[Value]) -> R<Called> {
    let name = f.name().strip_prefix("String.prototype.");
    match (f, name) {
        (Builtin::STRING_PROTOTYPE_TO_STRING | Builtin::STRING_PROTOTYPE_VALUE_OF, Some(name)) => {
            done_str(this_string_value(this, name)?)
        }
        // Every other method of String.prototype starts with `ToString(RequireObjectCoercible(
        // this))`, which can call back into a method (`{toString: $s.trim}` recurses until
        // the stack runs out): it runs here, in a small frame, before the method's own.
        (_, Some(name)) => {
            let s = this_string(this, name)?;
            prototype_function(f, name, s, args)
        }
        _ => static_function(f, args),
    }
}

/// `String`'s own functions.
#[inline(never)]
fn static_function(f: Builtin, args: &[Value]) -> R<Called> {
    let a0 = arg(args, 0);
    match f {
        Builtin::STRING => (|| -> R<Called> {
            if args.is_empty() {
                return done_str("");
            }
            done_str(string(&a0)?)
        })(),
        Builtin::STRING_FROM_CHAR_CODE => (|| -> R<Called> {
            let mut units = Vec::new();
            for a in args {
                units.push(uint16(a)?);
            }
            js(units)
        })(),
        Builtin::STRING_FROM_CODE_POINT => (|| -> R<Called> {
            let mut units = Vec::new();
            for a in args {
                let n = number(a)?;
                if n.is_nan() || n.trunc() != n || !(0.0..=1114111.0).contains(&n) {
                    return Err(range(format!(
                        "Invalid code point {}",
                        js::number_to_string(n)
                    )));
                }
                let mut buf = [0u16; 2];
                match char::from_u32(n as u32) {
                    Some(c) => units.extend_from_slice(c.encode_utf16(&mut buf)),
                    None => units.push(n as u16),
                }
            }
            js(units)
        })(),
        Builtin::STRING_RAW => (|| -> R<Called> {
            let cooked = object_value(&a0)?;
            let raw = get_property(&cooked, "raw")?;
            let literals = object_value(&raw)?;
            let count = len_of(&literals)?;
            if count <= 0.0 {
                return done_str("");
            }
            let mut out = Text::new();
            let mut i = 0.0;
            loop {
                out.push(&string(&get_property(
                    &literals,
                    &js::number_to_string(i),
                )?)?)?;
                if i + 1.0 >= count {
                    break;
                }
                if let Some(sub) = args.get(i as usize + 1) {
                    out.push(&string(sub)?)?;
                }
                i += 1.0;
            }
            done_str(out.finish())
        })(),
        other => Err(no_such_function(other)),
    }
}

/// A method of `String.prototype` (but `toString` and `valueOf`), `this` already converted:
/// `s`.
#[inline(never)]
fn prototype_function(f: Builtin, name: &str, s: Vec<u16>, args: &[Value]) -> R<Called> {
    let a0 = &arg(args, 0);
    let a1 = &arg(args, 1);
    match f {
        Builtin::STRING_PROTOTYPE_ANCHOR => return create_html(s, "a", Some(("name", a0))),
        Builtin::STRING_PROTOTYPE_BIG => return create_html(s, "big", None),
        Builtin::STRING_PROTOTYPE_BLINK => return create_html(s, "blink", None),
        Builtin::STRING_PROTOTYPE_BOLD => return create_html(s, "b", None),
        Builtin::STRING_PROTOTYPE_FIXED => return create_html(s, "tt", None),
        Builtin::STRING_PROTOTYPE_FONTCOLOR => {
            return create_html(s, "font", Some(("color", a0)));
        }
        Builtin::STRING_PROTOTYPE_FONTSIZE => {
            return create_html(s, "font", Some(("size", a0)));
        }
        Builtin::STRING_PROTOTYPE_ITALICS => return create_html(s, "i", None),
        Builtin::STRING_PROTOTYPE_LINK => return create_html(s, "a", Some(("href", a0))),
        Builtin::STRING_PROTOTYPE_SMALL => return create_html(s, "small", None),
        Builtin::STRING_PROTOTYPE_STRIKE => return create_html(s, "strike", None),
        Builtin::STRING_PROTOTYPE_SUB => return create_html(s, "sub", None),
        Builtin::STRING_PROTOTYPE_SUP => return create_html(s, "sup", None),
        _ => {}
    }
    let len = s.len();
    match f {
        Builtin::STRING_PROTOTYPE_AT => (|| -> R<Called> {
            let rel = integer(a0)?;
            let k = if rel >= 0.0 { rel } else { len as f64 + rel };
            if k < 0.0 || k >= len as f64 {
                return done(Value::Undefined);
            }
            done_str(JsString::from_code_unit(s[k as usize]))
        })(),
        Builtin::STRING_PROTOTYPE_CHAR_AT => (|| -> R<Called> {
            let p = integer(a0)?;
            if p < 0.0 || p >= len as f64 {
                return done_str("");
            }
            done_str(JsString::from_code_unit(s[p as usize]))
        })(),
        Builtin::STRING_PROTOTYPE_CHAR_CODE_AT => (|| -> R<Called> {
            let p = integer(a0)?;
            if p < 0.0 || p >= len as f64 {
                return done_num(f64::NAN);
            }
            done_num(f64::from(s[p as usize]))
        })(),
        Builtin::STRING_PROTOTYPE_CODE_POINT_AT => (|| -> R<Called> {
            let p = integer(a0)?;
            if p < 0.0 || p >= len as f64 {
                return done(Value::Undefined);
            }
            let cp = js::code_point_at(&s, p as usize).expect("in range");
            done_num(f64::from(cp))
        })(),
        Builtin::STRING_PROTOTYPE_CONCAT => (|| -> R<Called> {
            let mut out = Text::new();
            out.push(&JsString::from_utf16(&s))?;
            for a in args {
                out.push(&string(a)?)?;
            }
            done_str(out.finish())
        })(),
        Builtin::STRING_PROTOTYPE_ENDS_WITH
        | Builtin::STRING_PROTOTYPE_STARTS_WITH
        | Builtin::STRING_PROTOTYPE_INCLUDES => {
            if is_regexp(a0) {
                return Err(type_err(format!(
                    "First argument to String.prototype.{name} must not be a regular expression"
                )));
            }
            let search = string(a0)?.to_utf16().into_owned();
            match f {
                Builtin::STRING_PROTOTYPE_ENDS_WITH => {
                    let end = if a1.is_undefined() {
                        len
                    } else {
                        clamp(integer(a1)?, len)
                    };
                    done_bool(end >= search.len() && s[end - search.len()..end] == search[..])
                }
                Builtin::STRING_PROTOTYPE_STARTS_WITH => {
                    let start = clamp(integer(a1)?, len);
                    done_bool(
                        s.len() - start >= search.len()
                            && s[start..start + search.len()] == search[..],
                    )
                }
                _ => {
                    let start = clamp(integer(a1)?, len);
                    done_bool(js::index_of(&s, &search, start).is_some())
                }
            }
        }
        Builtin::STRING_PROTOTYPE_INDEX_OF => (|| -> R<Called> {
            let search = string(a0)?.to_utf16().into_owned();
            let start = clamp(integer(a1)?, len);
            done_num(match js::index_of(&s, &search, start) {
                Some(i) => i as f64,
                None => -1.0,
            })
        })(),
        Builtin::STRING_PROTOTYPE_LAST_INDEX_OF => (|| -> R<Called> {
            let search = string(a0)?.to_utf16().into_owned();
            let pos = number(a1)?;
            let start = if pos.is_nan() {
                len
            } else {
                clamp(pos.trunc(), len)
            };
            let max = len.saturating_sub(search.len());
            let from = start.min(max);
            if search.len() > len {
                return done_num(-1.0);
            }
            let mut i = from as isize;
            while i >= 0 {
                let k = i as usize;
                if s[k..k + search.len()] == search[..] {
                    return done_num(k as f64);
                }
                i -= 1;
            }
            done_num(-1.0)
        })(),
        Builtin::STRING_PROTOTYPE_IS_WELL_FORMED => {
            (|| -> R<Called> { done_bool(String::from_utf16(&s).is_ok()) })()
        }
        Builtin::STRING_PROTOTYPE_TO_WELL_FORMED => (|| -> R<Called> {
            let fixed: String = char::decode_utf16(s.iter().copied())
                .map(|c| c.unwrap_or('\u{FFFD}'))
                .collect();
            done_str(fixed)
        })(),
        Builtin::STRING_PROTOTYPE_LOCALE_COMPARE => (|| -> R<Called> {
            let that = string(a0)?.to_utf16().into_owned();
            done_num(locale_compare(&s, &that))
        })(),
        Builtin::STRING_PROTOTYPE_NORMALIZE => (|| -> R<Called> {
            let form = if a0.is_undefined() {
                "NFC".to_string()
            } else {
                string(a0)?.to_string_lossy().into_owned()
            };
            if !matches!(form.as_str(), "NFC" | "NFD" | "NFKC" | "NFKD") {
                return Err(range(
                    "The normalization form should be one of NFC, NFD, NFKC, NFKD.",
                ));
            }
            js(s)
        })(),
        Builtin::STRING_PROTOTYPE_PAD_END | Builtin::STRING_PROTOTYPE_PAD_START => {
            (|| -> R<Called> {
                let max = length(a0)?;
                if max <= len as f64 {
                    return js(s);
                }
                let fill: Vec<u16> = if a1.is_undefined() {
                    vec![u16::from(b' ')]
                } else {
                    string(a1)?.to_utf16().into_owned()
                };
                if fill.is_empty() {
                    return js(s);
                }
                if max > super::call::MAX_STRING_LENGTH as f64 {
                    return Err(range("Invalid string length"));
                }
                let fill_len = max as usize - len;
                let padding: Vec<u16> = fill.iter().copied().cycle().take(fill_len).collect();
                let out = if f == Builtin::STRING_PROTOTYPE_PAD_END {
                    [s, padding].concat()
                } else {
                    [padding, s].concat()
                };
                js(out)
            })()
        }
        Builtin::STRING_PROTOTYPE_REPEAT => (|| -> R<Called> {
            let n = integer(a0)?;
            if n < 0.0 || n == f64::INFINITY {
                return Err(range(format!(
                    "Invalid count value: {}",
                    js::number_to_string(number(a0)?)
                )));
            }
            if len == 0 || n == 0.0 {
                return done_str("");
            }
            if n * len as f64 > super::call::MAX_STRING_LENGTH as f64 {
                return Err(range("Invalid string length"));
            }
            js(s.repeat(n as usize))
        })(),
        Builtin::STRING_PROTOTYPE_REPLACE | Builtin::STRING_PROTOTYPE_REPLACE_ALL => {
            (|| -> R<Called> {
                if f == Builtin::STRING_PROTOTYPE_REPLACE_ALL && is_regexp(a0) {
                    return Err(type_err("replaceAll must be called with a global RegExp"));
                }
                let search = string(a0)?.to_utf16().into_owned();
                let functional = super::call::is_callable(a1);
                let replacement = if functional {
                    Vec::new()
                } else {
                    string(a1)?.to_utf16().into_owned()
                };
                let positions: Vec<usize> = if f == Builtin::STRING_PROTOTYPE_REPLACE {
                    js::index_of(&s, &search, 0).into_iter().collect()
                } else {
                    let advance = search.len().max(1);
                    let mut out = Vec::new();
                    let mut p = js::index_of(&s, &search, 0);
                    while let Some(found) = p {
                        out.push(found);
                        // StringIndexOf: not found once the position is past the end (an empty
                        // search matches at every position up to the length, once each).
                        let next = found + advance;
                        p = if next > s.len() {
                            None
                        } else {
                            js::index_of(&s, &search, next)
                        };
                    }
                    out
                };
                let mut out = Text::new();
                let mut end = 0;
                for p in positions {
                    out.push(&JsString::from_utf16(&s[end..p]))?;
                    let piece = if functional {
                        let r = super::call::call_value(
                            a1,
                            &Value::Undefined,
                            &[
                                Value::String(JsString::from_utf16(&search)),
                                Value::Number(p as f64),
                                Value::String(JsString::from_utf16(&s)),
                            ],
                        )?;
                        let v = match r {
                            Called::Value(v) | Called::Wrapper(v) => v,
                        };
                        string(&v)?.to_utf16().into_owned()
                    } else {
                        substitution(&search, &s, p, &replacement)
                    };
                    out.push(&JsString::from_utf16(&piece))?;
                    end = p + search.len();
                }
                out.push(&JsString::from_utf16(&s[end..]))?;
                done_str(out.finish())
            })()
        }
        Builtin::STRING_PROTOTYPE_SLICE => (|| -> R<Called> {
            let from = relative(a0, len, 0)?;
            let to = if a1.is_undefined() {
                len
            } else {
                relative(a1, len, len)?
            };
            js(if from < to {
                s[from..to].to_vec()
            } else {
                Vec::new()
            })
        })(),
        Builtin::STRING_PROTOTYPE_SUBSTRING => (|| -> R<Called> {
            let start = clamp(integer(a0)?, len);
            let end = if a1.is_undefined() {
                len
            } else {
                clamp(integer(a1)?, len)
            };
            let (from, to) = (start.min(end), start.max(end));
            js(s[from..to].to_vec())
        })(),
        Builtin::STRING_PROTOTYPE_SUBSTR => (|| -> R<Called> {
            let start = relative(a0, len, 0)?;
            let count = if a1.is_undefined() {
                len as f64
            } else {
                integer(a1)?
            };
            let end = (start as f64 + count).min(len as f64);
            if count <= 0.0 || start as f64 >= end {
                return done_str("");
            }
            js(s[start..end as usize].to_vec())
        })(),
        Builtin::STRING_PROTOTYPE_SPLIT => (|| -> R<Called> { split(&s, a0, a1) })(),
        Builtin::STRING_PROTOTYPE_TRIM => (|| -> R<Called> { js(js::trim(&s).to_vec()) })(),
        Builtin::STRING_PROTOTYPE_TRIM_START => {
            (|| -> R<Called> { js(js::trim_start(&s).to_vec()) })()
        }
        Builtin::STRING_PROTOTYPE_TRIM_END => (|| -> R<Called> { js(js::trim_end(&s).to_vec()) })(),
        Builtin::STRING_PROTOTYPE_TO_LOWER_CASE => {
            (|| -> R<Called> { js(js::to_lower_case(&s)) })()
        }
        Builtin::STRING_PROTOTYPE_TO_UPPER_CASE => {
            (|| -> R<Called> { js(js::to_upper_case(&s)) })()
        }
        Builtin::STRING_PROTOTYPE_TO_LOCALE_LOWER_CASE
        | Builtin::STRING_PROTOTYPE_TO_LOCALE_UPPER_CASE => {
            let language = locale_language(a0)?;
            js(locale_case(
                &s,
                language.as_deref(),
                f == Builtin::STRING_PROTOTYPE_TO_LOCALE_UPPER_CASE,
            ))
        }
        Builtin::STRING_PROTOTYPE_MATCH
        | Builtin::STRING_PROTOTYPE_MATCH_ALL
        | Builtin::STRING_PROTOTYPE_SEARCH => {
            let pattern = if a0.is_undefined() {
                String::new()
            } else {
                string(a0)?.to_string_lossy().into_owned()
            };
            let flags = if f == Builtin::STRING_PROTOTYPE_MATCH_ALL {
                "g"
            } else {
                ""
            };
            let rx = JsRegExp::new(&pattern, flags)?;
            match f {
                Builtin::STRING_PROTOTYPE_MATCH_ALL => Ok(opaque()),
                // (`exec` of a regex without `g` or `y`: from the start; V8's `RangeError`
                // where its backtracking runs out.)
                Builtin::STRING_PROTOTYPE_SEARCH => done_num(match rx.exec_utf16(&s)? {
                    Some(m) => m.range.start as f64,
                    None => -1.0,
                }),
                _ => done(match rx.exec_utf16(&s)? {
                    Some(m) => Value::array(
                        std::iter::once(Some(m.range))
                            .chain(m.captures)
                            .map(|g| match g {
                                Some(r) => Value::String(JsString::from_utf16(&s[r])),
                                None => Value::Undefined,
                            })
                            .collect(),
                    ),
                    None => Value::Null,
                }),
            }
        }
        other => Err(no_such_function(other)),
    }
}

/// A relative position (`slice`'s and `substr`'s start) clamped into `0..=len`.
fn relative(v: &Value, len: usize, default: usize) -> R<usize> {
    if v.is_undefined() {
        return Ok(default);
    }
    let rel = integer(v)?;
    Ok(if rel < 0.0 {
        (len as f64 + rel).max(0.0) as usize
    } else {
        clamp(rel, len)
    })
}

/// `IsRegExp(value)`: no value here is a RegExp.
fn is_regexp(_value: &Value) -> bool {
    false
}

/// `String.prototype.localeCompare` (see the module documentation of `call`: ICU's root
/// collation for ASCII text, code units beyond).
fn locale_compare(a: &[u16], b: &[u16]) -> f64 {
    if a.iter().chain(b).all(|&c| c < 0x80) {
        return super::collation::compare_ascii(a, b);
    }
    match a.cmp(b) {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    }
}

// ---------------------------------------------------------------------------------------
// RegExp
// ---------------------------------------------------------------------------------------

/// V8's `incompatible receiver` error for `RegExp.prototype.{method}`.
fn incompatible(method: &str, this: &Value) -> String {
    type_err(format!(
        "Method RegExp.prototype.{method} called on incompatible receiver {}",
        super::call::no_side_effects_to_string(this)
    ))
}

pub(super) fn regexp_function(f: Builtin, this: &Value, args: &[Value]) -> R<Called> {
    let a0 = arg(args, 0);
    match f {
        Builtin::REGEXP => (|| -> R<Called> {
            let pattern = if a0.is_undefined() {
                String::new()
            } else {
                string(&a0)?.to_string_lossy().into_owned()
            };
            let flags_arg = arg(args, 1);
            let flags = if flags_arg.is_undefined() {
                String::new()
            } else {
                string(&flags_arg)?.to_string_lossy().into_owned()
            };
            JsRegExp::new(&pattern, &flags)?;
            Ok(opaque())
        })(),
        Builtin::REGEXP_PROTOTYPE_EXEC => Err(incompatible("exec", this)),
        Builtin::REGEXP_PROTOTYPE_COMPILE => Err(incompatible("compile", this)),
        Builtin::REGEXP_PROTOTYPE_TEST => (|| -> R<Called> {
            if !is_object(this) {
                return Err(incompatible("test", this));
            }
            let s = string(&a0)?;
            // RegExpExec: a callable `exec` of the receiver's, else the built-in one, which
            // needs a RegExp.
            let exec = get_property(this, "exec")?;
            if super::call::is_callable(&exec) {
                let result = super::call::call_value(&exec, this, &[Value::String(s)])?;
                return match result {
                    Called::Value(Value::Null) => done_bool(false),
                    Called::Value(v) if is_object(&v) => done_bool(true),
                    Called::Wrapper(_) => done_bool(true),
                    _ => Err(type_err("object null is not a function")),
                };
            }
            Err(incompatible("exec", this))
        })(),
        Builtin::REGEXP_PROTOTYPE_TO_STRING => (|| -> R<Called> {
            if !is_object(this) {
                return Err(incompatible("toString", this));
            }
            let source = string(&get_property(this, "source")?)?;
            let flags = string(&get_property(this, "flags")?)?;
            let mut out = Text::new();
            out.push_str("/")?;
            out.push(&source)?;
            out.push_str("/")?;
            out.push(&flags)?;
            done_str(out.finish())
        })(),
        other => Err(no_such_function(other)),
    }
}
