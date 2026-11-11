//! Port of punycode.js 2.3.1 (punycode.es6.js, as bundled in upstream Markdoc 0.5.11).
//!
//! JS strings are UTF-16: `&[u16]` / `Vec<u16>`; the functions that throw `RangeError` in JS
//! return `Err` with the same message. JS numbers are f64; every quantity here is an integer
//! kept below 2^31 by upstream's own overflow checks, so `i64` holds them exactly, and
//! `floor(a / b)` is `floor_div` (it can see `a < 0` once, in `encode`).

use super::js::{self, RangeError};

/// `punycode.version`.
pub const VERSION: &str = "2.3.1";

/** Highest positive signed 32-bit float value */
const MAX_INT: i64 = 2147483647; // aka. 0x7FFFFFFF or 2^31-1

/* Bootstring parameters */
const BASE: i64 = 36;
const T_MIN: i64 = 1;
const T_MAX: i64 = 26;
const SKEW: i64 = 38;
const DAMP: i64 = 700;
const INITIAL_BIAS: i64 = 72;
const INITIAL_N: i64 = 128; // 0x80
const DELIMITER: u16 = 0x2D; // '\x2D'

/* Regular expressions */
// regexPunycode = /^xn--/                                   (`regex_punycode`)
// regexNonASCII = /[^\0-\x7F]/; // Note: U+007F DEL is excluded too.   (`regex_non_ascii`)
// regexSeparators = /[\x2E。．｡]/g; // RFC 3490 separators   (`is_separator`)

/* Convenience shortcuts */
const BASE_MINUS_T_MIN: i64 = BASE - T_MIN;

/// `Math.floor(a / b)` for `b > 0`.
fn floor_div(a: i64, b: i64) -> i64 {
    a.div_euclid(b)
}

/*--------------------------------------------------------------------------*/

/// `error(type)`: the `RangeError` upstream throws, with the message of `errors[type]`.
fn error(kind: &str) -> RangeError {
    /* Error messages */
    let message = match kind {
        "overflow" => "Overflow: input needs wider integers to process",
        "not-basic" => "Illegal input >= 0x80 (not a basic code point)",
        "invalid-input" => "Invalid input",
        _ => unreachable!("punycode error type {kind}"),
    };
    RangeError(message.to_owned())
}

/// `map(array, callback)`: a generic `Array#map` utility function. It runs from the last
/// element to the first, so when several callbacks would throw, the last element's error is
/// the one that comes back.
fn map(
    array: &[&[u16]],
    callback: fn(&[u16]) -> Result<Vec<u16>, RangeError>,
) -> Result<Vec<Vec<u16>>, RangeError> {
    let mut result: Vec<Vec<u16>> = vec![Vec::new(); array.len()];
    let mut length = array.len();
    while length > 0 {
        length -= 1;
        result[length] = callback(array[length])?;
    }
    Ok(result)
}

/// One of the RFC 3490 separators: `.` U+3002 U+FF0E U+FF61.
fn is_separator(c: u16) -> bool {
    matches!(c, 0x2E | 0x3002 | 0xFF0E | 0xFF61)
}

/// `mapDomain(domain, callback)`: a simple `Array#map`-like wrapper to work with domain name
/// strings or email addresses.
fn map_domain(
    domain: &[u16],
    callback: fn(&[u16]) -> Result<Vec<u16>, RangeError>,
) -> Result<Vec<u16>, RangeError> {
    let parts = js::split(domain, &[u16::from(b'@')]);
    let mut result: Vec<u16> = Vec::new();
    let mut domain = domain;
    if parts.len() > 1 {
        // In email addresses, only the domain name should be punycoded. Leave
        // the local part (i.e. everything up to `@`) intact.
        // (Anything after a second `@` is dropped: upstream's behavior.)
        result.extend_from_slice(parts[0]);
        result.push(u16::from(b'@'));
        domain = parts[1];
    }
    // Avoid `split(regex)` for IE8 compatibility. See #17.
    // domain = domain.replace(regexSeparators, '\x2E')
    let domain: Vec<u16> = domain
        .iter()
        .map(|&c| if is_separator(c) { 0x2E } else { c })
        .collect();
    let labels = js::split(&domain, &[0x2E]);
    let encoded = map(&labels, callback)?;
    for (i, label) in encoded.iter().enumerate() {
        if i > 0 {
            result.push(0x2E);
        }
        result.extend_from_slice(label);
    }
    Ok(result)
}

/// `punycode.ucs2.decode(string)`: the numeric code points of each Unicode character in the
/// string. A pair of surrogate halves becomes a single code point, matching UTF-16; an
/// unmatched surrogate stays as its own value.
pub fn ucs2_decode(string: &[u16]) -> Vec<u32> {
    let mut output: Vec<u32> = Vec::new();
    let mut counter = 0;
    let length = string.len();
    while counter < length {
        let value = u32::from(string[counter]);
        counter += 1;
        if value >= 0xD800 && value <= 0xDBFF && counter < length {
            // It's a high surrogate, and there is a next character.
            let extra = u32::from(string[counter]);
            counter += 1;
            if (extra & 0xFC00) == 0xDC00 {
                // Low surrogate.
                output.push(((value & 0x3FF) << 10) + (extra & 0x3FF) + 0x10000);
            } else {
                // It's an unmatched surrogate; only append this code unit, in case the
                // next code unit is the high surrogate of a surrogate pair.
                output.push(value);
                counter -= 1;
            }
        } else {
            output.push(value);
        }
    }
    output
}

/// `punycode.ucs2.encode(codePoints)`: `String.fromCodePoint(...codePoints)`, which throws a
/// `RangeError` past U+10FFFF.
pub fn ucs2_encode(code_points: &[u32]) -> Result<Vec<u16>, RangeError> {
    js::from_code_point(code_points)
}

/// `basicToDigit(codePoint)`: the numeric value of a basic code point (for use in
/// representing integers) in the range `0` to `base - 1`, or `base` if the code point does
/// not represent a value.
fn basic_to_digit(code_point: i64) -> i64 {
    if code_point >= 0x30 && code_point < 0x3A {
        return 26 + (code_point - 0x30);
    }
    if code_point >= 0x41 && code_point < 0x5B {
        return code_point - 0x41;
    }
    if code_point >= 0x61 && code_point < 0x7B {
        return code_point - 0x61;
    }
    BASE
}

/// `digitToBasic(digit, flag)`: the basic code point whose value (when used for representing
/// integers) is `digit`, which needs to be in the range `0` to `base - 1`. If `flag` is
/// non-zero, the uppercase form is used; else, the lowercase form is used.
fn digit_to_basic(digit: i64, flag: i64) -> i64 {
    //  0..25 map to ASCII a..z or A..Z
    // 26..35 map to ASCII 0..9
    digit + 22 + 75 * i64::from(digit < 26) - (i64::from(flag != 0) << 5)
}

/// `adapt(delta, numPoints, firstTime)`: bias adaptation function as per section 3.4 of
/// RFC 3492 (https://tools.ietf.org/html/rfc3492#section-3.4).
fn adapt(delta: i64, num_points: i64, first_time: bool) -> i64 {
    let mut k = 0;
    // `delta >> 1` is an int32 shift; `delta` is in 0..=maxInt here
    let mut delta = if first_time {
        floor_div(delta, DAMP)
    } else {
        delta >> 1
    };
    delta += floor_div(delta, num_points);
    // `baseMinusTMin * tMax >> 1` is `(35 * 26) >> 1`
    while delta > (BASE_MINUS_T_MIN * T_MAX) >> 1 {
        delta = floor_div(delta, BASE_MINUS_T_MIN);
        k += BASE;
    }
    // floor(k + (baseMinusTMin + 1) * delta / (delta + skew)), a float division in JS
    (k as f64 + ((BASE_MINUS_T_MIN + 1) * delta) as f64 / (delta + SKEW) as f64).floor() as i64
}

/// `t` of the Bootstring loops: `k <= bias ? tMin : (k >= bias + tMax ? tMax : k - bias)`.
fn threshold(k: i64, bias: i64) -> i64 {
    if k <= bias {
        T_MIN
    } else if k >= bias + T_MAX {
        T_MAX
    } else {
        k - bias
    }
}

/// `punycode.decode(input)`: a Punycode string of ASCII-only symbols to a string of Unicode
/// symbols.
pub fn decode(input: &[u16]) -> Result<Vec<u16>, RangeError> {
    // Don't use UCS-2.
    let mut output: Vec<u32> = Vec::new();
    let input_length = input.len() as i64;
    let mut i: i64 = 0;
    let mut n: i64 = INITIAL_N;
    let mut bias: i64 = INITIAL_BIAS;

    // Handle the basic code points: let `basic` be the number of input code
    // points before the last delimiter, or `0` if there is none, then copy
    // the first basic code points to the output.

    let mut basic: i64 = js::last_index_of(input, &[DELIMITER], None).map_or(-1, |b| b as i64);
    if basic < 0 {
        basic = 0;
    }

    for j in 0..basic as usize {
        // if it's not a basic code point
        if input[j] >= 0x80 {
            return Err(error("not-basic"));
        }
        output.push(u32::from(input[j]));
    }

    // Main decoding loop: start just after the last delimiter if any basic code
    // points were copied; start at the beginning otherwise.

    let mut index: i64 = if basic > 0 { basic + 1 } else { 0 };
    while index < input_length {
        // `index` is the index of the next character to be consumed.
        // Decode a generalized variable-length integer into `delta`,
        // which gets added to `i`. The overflow checking is easier
        // if we increase `i` as we go, then subtract off its starting
        // value at the end to obtain `delta`.
        let oldi = i;
        let mut w: i64 = 1;
        let mut k: i64 = BASE;
        loop {
            if index >= input_length {
                return Err(error("invalid-input"));
            }

            let digit = basic_to_digit(i64::from(input[index as usize]));
            index += 1;

            if digit >= BASE {
                return Err(error("invalid-input"));
            }
            if digit > floor_div(MAX_INT - i, w) {
                return Err(error("overflow"));
            }

            i += digit * w;
            let t = threshold(k, bias);

            if digit < t {
                break;
            }

            let base_minus_t = BASE - t;
            if w > floor_div(MAX_INT, base_minus_t) {
                return Err(error("overflow"));
            }

            w *= base_minus_t;
            k += BASE;
        }

        let out = output.len() as i64 + 1;
        bias = adapt(i - oldi, out, oldi == 0);

        // `i` was supposed to wrap around from `out` to `0`,
        // incrementing `n` each time, so we'll fix that now:
        if floor_div(i, out) > MAX_INT - n {
            return Err(error("overflow"));
        }

        n += floor_div(i, out);
        i %= out;

        // Insert `n` at position `i` of the output.
        // (`n` stays within 0..=maxInt, so it fits a u32)
        output.insert(i as usize, n as u32);
        i += 1;
    }

    // String.fromCodePoint(...output): a RangeError past U+10FFFF
    js::from_code_point(&output)
}

/// `punycode.encode(input)`: a string of Unicode symbols (e.g. a domain name label) to a
/// Punycode string of ASCII-only symbols.
pub fn encode(input: &[u16]) -> Result<Vec<u16>, RangeError> {
    // JS collects one-character strings and joins them; one code unit each here
    let mut output: Vec<u16> = Vec::new();

    // Convert the input in UCS-2 to an array of Unicode code points.
    let input: Vec<i64> = ucs2_decode(input).into_iter().map(i64::from).collect();

    // Cache the length.
    let input_length = input.len() as i64;

    // Initialize the state.
    let mut n: i64 = INITIAL_N;
    let mut delta: i64 = 0;
    let mut bias: i64 = INITIAL_BIAS;

    // Handle the basic code points.
    for &current_value in &input {
        if current_value < 0x80 {
            // stringFromCharCode(currentValue)
            output.push(current_value as u16);
        }
    }

    let basic_length = output.len() as i64;
    let mut handled_cp_count = basic_length;

    // `handledCPCount` is the number of code points that have been handled;
    // `basicLength` is the number of basic code points.

    // Finish the basic string with a delimiter unless it's empty.
    if basic_length != 0 {
        output.push(DELIMITER);
    }

    // Main encoding loop:
    while handled_cp_count < input_length {
        // All non-basic code points < n have been handled already. Find the next
        // larger one:
        let mut m = MAX_INT;
        for &current_value in &input {
            if current_value >= n && current_value < m {
                m = current_value;
            }
        }

        // Increase `delta` enough to advance the decoder's <n,i> state to <m,0>,
        // but guard against overflow.
        let handled_cp_count_plus_one = handled_cp_count + 1;
        if m - n > floor_div(MAX_INT - delta, handled_cp_count_plus_one) {
            return Err(error("overflow"));
        }

        delta += (m - n) * handled_cp_count_plus_one;
        n = m;

        for &current_value in &input {
            // `currentValue < n && ++delta > maxInt`
            if current_value < n && {
                delta += 1;
                delta > MAX_INT
            } {
                return Err(error("overflow"));
            }
            if current_value == n {
                // Represent delta as a generalized variable-length integer.
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = threshold(k, bias);
                    if q < t {
                        break;
                    }
                    let q_minus_t = q - t;
                    let base_minus_t = BASE - t;
                    output.push(digit_to_basic(t + q_minus_t % base_minus_t, 0) as u16);
                    q = floor_div(q_minus_t, base_minus_t);
                    k += BASE;
                }

                output.push(digit_to_basic(q, 0) as u16);
                bias = adapt(
                    delta,
                    handled_cp_count_plus_one,
                    handled_cp_count == basic_length,
                );
                delta = 0;
                handled_cp_count += 1;
            }
        }

        delta += 1;
        n += 1;
    }
    Ok(output)
}

/// `regexPunycode.test(string)`: `/^xn--/` (case-sensitive).
fn regex_punycode(string: &[u16]) -> bool {
    string.starts_with(&js::to_utf16("xn--"))
}

/// `regexNonASCII.test(string)`: `/[^\0-\x7F]/`.
fn regex_non_ascii(string: &[u16]) -> bool {
    string.iter().any(|&c| c > 0x7F)
}

/// `punycode.toUnicode(input)`: a Punycode string representing a domain name or an email
/// address to Unicode. Only the Punycoded parts of the input will be converted, i.e. it
/// doesn't matter if you call it on a string that has already been converted to Unicode.
pub fn to_unicode(input: &[u16]) -> Result<Vec<u16>, RangeError> {
    map_domain(input, |string| {
        if regex_punycode(string) {
            decode(&js::to_lower_case(js::slice(string, 4, None)))
        } else {
            Ok(string.to_vec())
        }
    })
}

/// `punycode.toASCII(input)`: a Unicode string representing a domain name or an email
/// address to Punycode. Only the non-ASCII parts of the domain name will be converted, i.e.
/// it doesn't matter if you call it with a domain that's already in ASCII.
pub fn to_ascii(input: &[u16]) -> Result<Vec<u16>, RangeError> {
    map_domain(input, |string| {
        if regex_non_ascii(string) {
            let mut out = js::to_utf16("xn--");
            out.extend(encode(string)?);
            Ok(out)
        } else {
            Ok(string.to_vec())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown_it::mdurl::gen_cases::{Function, Syms, check};

    #[test]
    fn generated_urls_match_node() {
        let functions: [Function; 7] = [
            ("puny_to_ascii", |o, s| {
                o.result(to_ascii(s));
            }),
            ("puny_to_unicode", |o, s| {
                o.result(to_unicode(s));
            }),
            ("puny_encode", |o, s| {
                o.result(encode(s));
            }),
            ("puny_decode", |o, s| {
                o.result(decode(s));
            }),
            ("puny_roundtrip", |o, s| {
                o.result(encode(s).and_then(|e| decode(&e)));
            }),
            ("puny_ucs2", |o: &mut Syms, s| {
                let cps = ucs2_decode(s);
                o.0.push(cps.len() as u32);
                o.0.extend(&cps);
                o.result(ucs2_encode(&cps));
            }),
            ("puny_host", |o, s| {
                let Some(h) = crate::markdown_it::mdurl::parse(s, true).hostname else {
                    o.0.push(crate::markdown_it::mdurl::gen_cases::NULL);
                    return;
                };
                let a = o.result(to_ascii(&h));
                o.result(to_unicode(&h));
                if let Some(a) = a {
                    o.result(to_unicode(&a));
                }
            }),
        ];
        check(&functions);
    }

    fn u(s: &str) -> Vec<u16> {
        js::to_utf16(s)
    }

    fn r(x: Result<Vec<u16>, RangeError>) -> String {
        match x {
            Ok(v) => js::to_string(&v),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn examples_and_errors() {
        // node: the bundled punycode.js (tests/markdown_it/core_values.cjs --examples)
        assert_eq!(r(to_ascii(&u("münchen.de"))), "xn--mnchen-3ya.de");
        assert_eq!(r(to_ascii(&u("user@ü.\u{3002}x"))), "user@xn--tda..x");
        assert_eq!(r(to_ascii(&u("a@b@ä"))), "a@b");
        assert_eq!(r(to_unicode(&u("xn--mnchen-3ya.de"))), "münchen.de");
        assert_eq!(
            r(to_unicode(&u("XN--mnchen-3ya.xn--MNCHEN-3YA"))),
            "XN--mnchen-3ya.münchen"
        );
        assert_eq!(
            r(decode(&u("99999999999"))),
            "RangeError: Overflow: input needs wider integers to process"
        );
        assert_eq!(
            r(decode(&u("ä-a"))),
            "RangeError: Illegal input >= 0x80 (not a basic code point)"
        );
        assert_eq!(r(decode(&u("a-"))), "a");
        assert_eq!(r(decode(&u("!"))), "RangeError: Invalid input");
        assert_eq!(r(encode(&[0xD83D, 0xDE00, 0xD800])), "ib9bt343c");
        assert_eq!(r(decode(&u("ls8h"))), "\u{1F4A9}");
        // the labels are mapped from the last one: its error is the one thrown
        assert_eq!(
            r(to_unicode(&u("xn--!.xn--99999999999"))),
            "RangeError: Overflow: input needs wider integers to process"
        );
        assert_eq!(r(decode(&u("hzzzzz"))), "RangeError: Invalid input");
        assert_eq!(
            r(ucs2_encode(&[0x41, 0x110000])),
            "RangeError: Invalid code point 1114112"
        );
        assert_eq!(
            ucs2_decode(&[0x61, 0xD83D, 0xDE00, 0xD800, 0xDBFF, 0xDC00]),
            [0x61, 0x1F600, 0xD800, 0x10FC00]
        );
    }
}
