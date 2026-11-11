//! The pieces of JavaScript's `JSON.stringify` and `Number.prototype.toString` that the
//! grammar's results and errors need, with JavaScript's exact output.
//!
//! `to_json_string()` on the grammar's types writes what `JSON.stringify` writes for the
//! JavaScript objects, byte for byte; `to_json()` builds the `serde_json::Value` that
//! `JSON.parse` of that text gives, as far as `serde_json::Value` can hold it (see
//! [`number_to_json`] and [`utf16_to_json`]).

use std::fmt::Write as _;

/// `Number::toString(x)` (ECMA-262 §6.1.6.1.20) in radix 10, which is what `String(x)` and,
/// for finite numbers, `JSON.stringify(x)` write.
pub(crate) fn number_to_string(x: f64) -> String {
    // 1. If x is NaN, return "NaN".
    if x.is_nan() {
        return "NaN".to_string();
    }
    // 2. If x is +0 or -0, return "0".
    if x == 0.0 {
        return "0".to_string();
    }
    // 3. If x < 0, return the string-concatenation of "-" and Number::toString(-x).
    if x < 0.0 {
        return format!("-{}", number_to_string(-x));
    }
    // 4. If x is +Infinity, return "Infinity".
    if x.is_infinite() {
        return "Infinity".to_string();
    }
    // 5. n, k and s: the fewest digits s (k of them) with s × 10^(n-k) = x.
    let (digits, n) = shortest_digits(x);
    let k = digits.len() as i64;
    if k <= n && n <= 21 {
        // 6. The k digits of s followed by n - k zeros.
        let mut out = digits;
        out.extend(std::iter::repeat_n('0', (n - k) as usize));
        out
    } else if 0 < n && n <= 21 {
        // 7. The first n digits, ".", the remaining k - n digits.
        let n = n as usize;
        format!("{}.{}", &digits[..n], &digits[n..])
    } else if -6 < n && n <= 0 {
        // 8. "0.", -n zeros, the k digits.
        format!("0.{}{}", "0".repeat((-n) as usize), digits)
    } else {
        // 9 and 10: exponential notation with an explicit sign.
        let sign = if n - 1 < 0 { '-' } else { '+' };
        let e = (n - 1).abs();
        if k == 1 {
            format!("{digits}e{sign}{e}")
        } else {
            format!("{}.{}e{sign}{e}", &digits[..1], &digits[1..])
        }
    }
}

/// Step 5 of `Number::toString` as V8 does it (and as the specification's note recommends):
/// the shortest digit string that reads back as x, the closest to x of those, and of two
/// equally close the even one. Returns the digits and n (x = 0.digits × 10^n).
///
/// Rust's `{:e}` writes the shortest, closest digits too, but rounds a tie up: for
/// 9041039839299.8125, exactly halfway between ...812 and ...813, it writes ...813 where
/// JavaScript writes ...812. A tie means x is exactly (c + 1/2) × 10^(n-k) for the k-digit c
/// below it, which `{:.*e}` with k digits after the point shows as c followed by a 5, and
/// [`equals_decimal`] confirms exactly; then the even one of c and c + 1 is taken, but only
/// if it reads back as x. The rule picks the even one of two candidates that both read back
/// as x, and at a power of two they need not: the double below 2^-24 is half as far away as
/// the one above, so of 5.960464477539062e-8 and ...063, equally close to 2^-24, only ...063
/// reads back as it (...062 is 0x1.fffffffffffffp-25), and V8 prints ...063, which is what
/// `{:e}` wrote.
fn shortest_digits(x: f64) -> (String, i64) {
    let sci = format!("{x:e}");
    let (mantissa, exponent) = sci
        .split_once('e')
        .expect("`{:e}` always writes an exponent");
    let digits: String = mantissa.chars().filter(|&c| c != '.').collect();
    let n = exponent
        .parse::<i64>()
        .expect("`{:e}` writes an integer exponent")
        + 1;
    let k = digits.len();
    if k > 18 {
        return (digits, n);
    }

    let wider = format!("{x:.k$e}");
    let (mantissa, exponent) = wider
        .split_once('e')
        .expect("`{:e}` always writes an exponent");
    let wider_digits: String = mantissa.chars().filter(|&c| c != '.').collect();
    let wider_n = exponent
        .parse::<i64>()
        .expect("`{:e}` writes an integer exponent")
        + 1;
    if wider_n != n || !wider_digits.ends_with('5') {
        return (digits, n);
    }
    let halfway: u64 = wider_digits.parse().expect("at most 19 decimal digits");
    if !equals_decimal(x, halfway, n - k as i64 - 1) {
        return (digits, n);
    }
    let below = halfway / 10;
    let even = if below.is_multiple_of(2) {
        below
    } else {
        below + 1
    };
    let mut even_digits = even.to_string();
    let mut even_n = n;
    if even_digits.len() > k {
        even_n += 1; // c + 1 carried into a new leading digit
    }
    while even_digits.len() > 1 && even_digits.ends_with('0') {
        even_digits.pop();
    }
    // The even candidate must read back as x (Rust's parse rounds correctly); where it does
    // not, the other one, which `{:e}` wrote, is the only candidate.
    if format!("0.{even_digits}e{even_n}").parse::<f64>() != Ok(x) {
        return (digits, n);
    }
    (even_digits, even_n)
}

/// Whether the finite positive x is exactly `odd × 10^exp`, for an odd `odd` (a decimal
/// ending in 5). With x = m × 2^e for an odd m, that is e = exp and m = odd × 5^exp
/// (exp >= 0) or m × 5^-exp = odd (exp < 0).
fn equals_decimal(x: f64, odd: u64, exp: i64) -> bool {
    let bits = x.to_bits();
    let biased = ((bits >> 52) & 0x7ff) as i64;
    let fraction = bits & ((1u64 << 52) - 1);
    let (mut m, mut e) = if biased == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1u64 << 52), biased - 1075)
    };
    if m == 0 || odd.is_multiple_of(2) {
        return false;
    }
    let zeros = m.trailing_zeros();
    m >>= zeros;
    e += i64::from(zeros);
    if e != exp {
        return false;
    }
    let power = |p: i64| -> Option<u128> {
        let mut out: u128 = 1;
        for _ in 0..p {
            out = out.checked_mul(5)?;
        }
        Some(out)
    };
    if exp >= 0 {
        power(exp)
            .and_then(|p| p.checked_mul(u128::from(odd)))
            .is_some_and(|v| v == u128::from(m))
    } else {
        power(-exp)
            .and_then(|p| p.checked_mul(u128::from(m)))
            .is_some_and(|v| v == u128::from(odd))
    }
}

/// A number as `JSON.stringify` writes it: `String(x)`, or `null` when x is not finite.
pub(crate) fn write_number(out: &mut String, x: f64) {
    if x.is_finite() {
        out.push_str(&number_to_string(x));
    } else {
        out.push_str("null");
    }
}

/// A number as `JSON.parse(JSON.stringify(x))` gives it, in a `serde_json::Value`.
///
/// Non-finite numbers are `null` (what `JSON.stringify` writes). A number whose JavaScript
/// text is a plain integer that fits `i64` or `u64` (every safe integer, -0 as 0, and the
/// larger whole numbers JavaScript prints in full, such as 1152921504606847000) is held as
/// that integer, so `serde_json` writes the same text. Every other number is held as the
/// f64; `serde_json` then writes its own spelling (`1e21` where JavaScript writes `1e+21`,
/// `1e-6` for `0.000001`), which reads back as the same number.
pub(crate) fn number_to_json(x: f64) -> serde_json::Value {
    if !x.is_finite() {
        return serde_json::Value::Null;
    }
    let text = number_to_string(x);
    if let Ok(i) = text.parse::<i64>() {
        return i.into();
    }
    if let Ok(u) = text.parse::<u64>() {
        return u.into();
    }
    serde_json::Number::from_f64(x).map_or(serde_json::Value::Null, serde_json::Value::Number)
}

/// One code point of `QuoteJSONString` (ECMA-262 §25.5.2.3).
fn quote_char(out: &mut String, c: char) {
    match c {
        '\u{8}' => out.push_str("\\b"),
        '\t' => out.push_str("\\t"),
        '\n' => out.push_str("\\n"),
        '\u{c}' => out.push_str("\\f"),
        '\r' => out.push_str("\\r"),
        '"' => out.push_str("\\\""),
        '\\' => out.push_str("\\\\"),
        c if (c as u32) < 0x20 => {
            let _ = write!(out, "\\u{:04x}", c as u32);
        }
        c => out.push(c),
    }
}

/// A string as `JSON.stringify` writes it.
pub(crate) fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        quote_char(out, c);
    }
    out.push('"');
}

/// A JavaScript string (UTF-16 code units) as `JSON.stringify` writes it: a surrogate that
/// is not half of a pair is written as a lowercase `\uXXXX` escape.
pub(crate) fn write_utf16(out: &mut String, s: &[u16]) {
    out.push('"');
    for unit in char::decode_utf16(s.iter().copied()) {
        match unit {
            Ok(c) => quote_char(out, c),
            Err(lone) => {
                let _ = write!(out, "\\u{:04x}", lone.unpaired_surrogate());
            }
        }
    }
    out.push('"');
}

/// A JavaScript string in a `serde_json::Value`. A Rust string cannot hold a lone
/// surrogate, so one is replaced by U+FFFD here; `write_utf16` keeps it.
pub(crate) fn utf16_to_json(s: &[u16]) -> serde_json::Value {
    serde_json::Value::String(String::from_utf16_lossy(s))
}
