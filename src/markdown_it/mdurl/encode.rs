//! Port of mdurl/lib/encode.mjs (mdurl 2.x, as bundled in upstream Markdoc 0.5.11).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::object_prototype_has;
use crate::markdown_it::js;

/// `encode.defaultChars`.
pub const DEFAULT_CHARS: &str = ";/?:@&=+$,-_.!~*'()#";
/// `encode.componentChars`.
pub const COMPONENT_CHARS: &str = "-_.!~*'()";

/// `'%' + ('0' + i.toString(16).toUpperCase()).slice(-2)` for a byte.
pub(super) fn percent_byte(i: u32) -> Vec<u16> {
    js::to_utf16(&format!("%{i:02X}"))
}

/// What `cache[code]` gives for each ASCII code; `None` is `undefined`.
type Cache = Vec<Option<Vec<u16>>>;

thread_local! {
    /// `encodeCache`: the tables `getEncodeCache` built, by `exclude`.
    static ENCODE_CACHE: RefCell<HashMap<Vec<u16>, Rc<Cache>>> = RefCell::new(HashMap::new());
}

// Create a lookup array where anything but characters in `chars` string
// and alphanumeric chars is percent-encoded.
//
/// `getEncodeCache(exclude)`.
fn get_encode_cache(exclude: &[u16]) -> Rc<Cache> {
    // `let cache = encodeCache[exclude]; if (cache) { return cache }` is a lookup on a plain
    // object: for a name that `Object.prototype` has (`toString`, `__proto__`, ...) it finds
    // that member, which is truthy and so is returned as the "cache"; indexing it gives
    // `undefined` for every code. Upstream's quirk, kept.
    if object_prototype_has(exclude) {
        return Rc::new(vec![None; 128]);
    }
    if let Some(cache) = ENCODE_CACHE.with_borrow(|memo| memo.get(exclude).cloned()) {
        return cache;
    }

    let mut cache: Cache = Vec::with_capacity(128);

    for i in 0..128u32 {
        let ch = i as u16;

        // /^[0-9a-z]$/i.test(ch): ASCII only (without the `u` flag, `i` folds no other
        // character onto an ASCII letter)
        if (ch as u8).is_ascii_alphanumeric() {
            // always allow unencoded alphanumeric characters
            cache.push(Some(vec![ch]));
        } else {
            cache.push(Some(percent_byte(i)));
        }
    }

    for i in 0..exclude.len() {
        // `cache[exclude.charCodeAt(i)] = exclude[i]`; codes past 127 only grow the JS
        // array, and `encode` reads `cache[code]` for `code < 128` alone
        if let Some(slot) = cache.get_mut(usize::from(exclude[i])) {
            *slot = Some(vec![exclude[i]]);
        }
    }

    let cache = Rc::new(cache);
    ENCODE_CACHE.with_borrow_mut(|memo| memo.insert(exclude.to_vec(), Rc::clone(&cache)));
    cache
}

/// `encodeURIComponent(s)` for the one character (BMP, or a surrogate pair) `encode`
/// passes it: the unreserved ASCII characters stay, everything else becomes the
/// percent-encoded bytes of its UTF-8 form.
fn encode_uri_component(s: &[u16]) -> Vec<u16> {
    let mut out = Vec::new();
    for c in char::decode_utf16(s.iter().copied()) {
        // `encode` never passes a lone surrogate (it writes %EF%BF%BD for those itself);
        // JS would throw URIError
        let c = c.expect("URIError: URI malformed");
        if c.is_ascii_alphanumeric() || "-_.!~*'()".contains(c) {
            out.push(c as u16);
            continue;
        }
        let mut buf = [0u8; 4];
        for &b in c.encode_utf8(&mut buf).as_bytes() {
            out.extend(percent_byte(u32::from(b)));
        }
    }
    out
}

fn is_hex_digit(c: u16) -> bool {
    c < 0x80 && (c as u8).is_ascii_hexdigit()
}

// Encode unsafe characters with percent-encoding, skipping already
// encoded sequences.
//
//  - string       - string to encode
//  - exclude      - list of characters to ignore (in addition to a-zA-Z0-9)
//  - keepEscaped  - don't encode '%' in a correct escape sequence (default: true)
//
/// `encode(string, exclude, keepEscaped)`: percent-encode unsafe characters, skipping
/// already encoded sequences. `exclude: None` is `encode.defaultChars` (which also covers the
/// `encode(string, keepEscaped)` overload); `keep_escaped: None` is `true`.
pub fn encode(string: &[u16], exclude: Option<&str>, keep_escaped: Option<bool>) -> Vec<u16> {
    // `typeof exclude !== 'string'`: `encode(string, keepEscaped)`, given here as
    // `exclude: None`
    let exclude = js::to_utf16(exclude.unwrap_or(DEFAULT_CHARS));

    // `typeof keepEscaped === 'undefined'`
    let keep_escaped = keep_escaped.unwrap_or(true);

    let cache = get_encode_cache(&exclude);
    let mut result: Vec<u16> = Vec::new();

    let l = string.len();
    let mut i = 0;
    // `for (let i = 0, l = string.length; i < l; i++)`: a `continue` there is a
    // `break 'body` here, which still runs the `i += 1`
    while i < l {
        'body: {
            let code = string[i];

            if keep_escaped && code == 0x25 /* % */ && i + 2 < l {
                // /^[0-9a-f]{2}$/i.test(string.slice(i + 1, i + 3))
                if is_hex_digit(string[i + 1]) && is_hex_digit(string[i + 2]) {
                    result.extend_from_slice(&string[i..i + 3]);
                    i += 2;
                    break 'body;
                }
            }

            if code < 128 {
                // `result += cache[code]`: `undefined` appends the text "undefined"
                match &cache[usize::from(code)] {
                    Some(s) => result.extend_from_slice(s),
                    None => result.extend(js::to_utf16("undefined")),
                }
                break 'body;
            }

            if code >= 0xD800 && code <= 0xDFFF {
                if code >= 0xD800 && code <= 0xDBFF && i + 1 < l {
                    let next_code = string[i + 1];
                    if next_code >= 0xDC00 && next_code <= 0xDFFF {
                        result.extend(encode_uri_component(&string[i..i + 2]));
                        i += 1;
                        break 'body;
                    }
                }
                result.extend(js::to_utf16("%EF%BF%BD"));
                break 'body;
            }

            result.extend(encode_uri_component(&string[i..i + 1]));
        }
        i += 1;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(s: &str, exclude: Option<&str>, keep: Option<bool>) -> String {
        js::to_string(&encode(&js::to_utf16(s), exclude, keep))
    }

    #[test]
    fn encode_matches_mdurl() {
        // values from node (tests/markdown_it/mdurl_values.cjs --examples)
        assert_eq!(enc("a b%20c%zz%", None, None), "a%20b%20c%25zz%25");
        assert_eq!(enc("%41%4", None, Some(false)), "%2541%254");
        assert_eq!(enc("ä€😀", None, None), "%C3%A4%E2%82%AC%F0%9F%98%80");
        assert_eq!(enc("/?#[]", Some(COMPONENT_CHARS), None), "%2F%3F%23%5B%5D");
        assert_eq!(
            enc("-_.!~*'()", Some(""), None),
            "%2D%5F%2E%21%7E%2A%27%28%29"
        );
        let lone = encode(&[0xD800, 0x41, 0xDC00, 0xDBFF], None, None);
        assert_eq!(js::to_string(&lone), "%EF%BF%BDA%EF%BF%BD%EF%BF%BD");
        // an exclude naming an Object.prototype member: the "cache" gives `undefined`
        assert_eq!(enc("a%41ä", Some("toString"), None), "undefined%41%C3%A4");
        assert_eq!(enc("ab", Some("__proto__"), None), "undefinedundefined");
    }
}
