//! Port of mdurl/lib/decode.mjs (mdurl 2.x, as bundled in upstream Markdoc 0.5.11).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::encode::percent_byte;
use super::object_prototype_has;
use crate::markdown_it::js;

/// `decode.defaultChars`.
pub const DEFAULT_CHARS: &str = ";/?:@&=+$,#";
/// `decode.componentChars`.
pub const COMPONENT_CHARS: &str = "";

/// What `cache[b]` gives for each byte below 0x80; `None` is `undefined`.
type Cache = Vec<Option<Vec<u16>>>;

thread_local! {
    /// `decodeCache`: the tables `getDecodeCache` built, by `exclude`.
    static DECODE_CACHE: RefCell<HashMap<Vec<u16>, Rc<Cache>>> = RefCell::new(HashMap::new());
}

/// `getDecodeCache(exclude)`.
fn get_decode_cache(exclude: &[u16]) -> Rc<Cache> {
    // `decodeCache[exclude]`: as in encode.rs, a name that `Object.prototype` has finds that
    // member, which is returned as a "cache" whose entries are all `undefined`
    if object_prototype_has(exclude) {
        return Rc::new(vec![None; 128]);
    }
    if let Some(cache) = DECODE_CACHE.with_borrow(|memo| memo.get(exclude).cloned()) {
        return cache;
    }

    let mut cache: Cache = Vec::with_capacity(128);

    for i in 0..128u16 {
        // String.fromCharCode(i)
        cache.push(Some(vec![i]));
    }

    for i in 0..exclude.len() {
        let ch = exclude[i];
        // `cache[ch] = '%' + ('0' + ch.toString(16).toUpperCase()).slice(-2)`; codes past
        // 127 only grow the JS array, and `decode` reads `cache[b1]` for `b1 < 0x80` alone
        if let Some(slot) = cache.get_mut(usize::from(ch)) {
            *slot = Some(percent_byte(u32::from(ch)));
        }
    }

    let cache = Rc::new(cache);
    DECODE_CACHE.with_borrow_mut(|memo| memo.insert(exclude.to_vec(), Rc::clone(&cache)));
    cache
}

fn hex_value(c: u16) -> Option<u32> {
    char::from_u32(u32::from(c))
        .filter(char::is_ascii_hexdigit)
        .and_then(|c| c.to_digit(16))
}

/// `parseInt(seq.slice(at, at + 2), 16)`, for the two hex digits of a `%XX` in `seq`.
fn byte_at(seq: &[u16], at: usize) -> u32 {
    hex_value(seq[at]).expect("hex digit") * 16 + hex_value(seq[at + 1]).expect("hex digit")
}

/// The length of the match of `/(%[a-f0-9]{2})+/i` at `pos` (0 when none): the longest run
/// of `%XX` triples (ASCII hex digits only: without the `u` flag, `i` folds no other
/// character onto them).
fn escapes_at(string: &[u16], pos: usize) -> usize {
    let mut end = pos;
    while end + 3 <= string.len()
        && string[end] == 0x25 /* % */
        && hex_value(string[end + 1]).is_some()
        && hex_value(string[end + 2]).is_some()
    {
        end += 3;
    }
    end - pos
}

// Decode percent-encoded string.
//
/// `decode(string, exclude)`: decode percent-encoded UTF-8 sequences, keeping the
/// characters in `exclude` encoded. `exclude: None` is `decode.defaultChars`.
pub fn decode(string: &[u16], exclude: Option<&str>) -> Vec<u16> {
    // `typeof exclude !== 'string'`
    let exclude = js::to_utf16(exclude.unwrap_or(DEFAULT_CHARS));

    let cache = get_decode_cache(&exclude);

    // string.replace(/(%[a-f0-9]{2})+/gi, function (seq) { ... })
    let mut out: Vec<u16> = Vec::with_capacity(string.len());
    let mut pos = 0;
    while pos < string.len() {
        let len = escapes_at(string, pos);
        if len == 0 {
            out.push(string[pos]);
            pos += 1;
            continue;
        }
        out.extend(decode_seq(&string[pos..pos + len], &cache));
        pos += len;
    }
    out
}

/// The replacement function of `decode`, for one run `seq` of `%XX` triples.
fn decode_seq(seq: &[u16], cache: &[Option<Vec<u16>>]) -> Vec<u16> {
    let mut result: Vec<u16> = Vec::new();

    let l = seq.len();
    let mut i = 0;
    // `for (let i = 0, l = seq.length; i < l; i += 3)`: a `continue` there is a
    // `break 'body` here, which still runs the `i += 3`
    while i < l {
        'body: {
            let b1 = byte_at(seq, i + 1);

            if b1 < 0x80 {
                // `result += cache[b1]`: `undefined` appends the text "undefined"
                match &cache[b1 as usize] {
                    Some(s) => result.extend_from_slice(s),
                    None => result.extend(js::to_utf16("undefined")),
                }
                break 'body;
            }

            if (b1 & 0xE0) == 0xC0 && (i + 3 < l) {
                // 110xxxxx 10xxxxxx
                let b2 = byte_at(seq, i + 4);

                if (b2 & 0xC0) == 0x80 {
                    let chr = ((b1 << 6) & 0x7C0) | (b2 & 0x3F);

                    if chr < 0x80 {
                        result.extend([0xFFFD, 0xFFFD]);
                    } else {
                        result.push(chr as u16);
                    }

                    i += 3;
                    break 'body;
                }
            }

            if (b1 & 0xF0) == 0xE0 && (i + 6 < l) {
                // 1110xxxx 10xxxxxx 10xxxxxx
                let b2 = byte_at(seq, i + 4);
                let b3 = byte_at(seq, i + 7);

                if (b2 & 0xC0) == 0x80 && (b3 & 0xC0) == 0x80 {
                    let chr = ((b1 << 12) & 0xF000) | ((b2 << 6) & 0xFC0) | (b3 & 0x3F);

                    if chr < 0x800 || (chr >= 0xD800 && chr <= 0xDFFF) {
                        result.extend([0xFFFD, 0xFFFD, 0xFFFD]);
                    } else {
                        result.push(chr as u16);
                    }

                    i += 6;
                    break 'body;
                }
            }

            if (b1 & 0xF8) == 0xF0 && (i + 9 < l) {
                // 111110xx 10xxxxxx 10xxxxxx 10xxxxxx
                let b2 = byte_at(seq, i + 4);
                let b3 = byte_at(seq, i + 7);
                let b4 = byte_at(seq, i + 10);

                if (b2 & 0xC0) == 0x80 && (b3 & 0xC0) == 0x80 && (b4 & 0xC0) == 0x80 {
                    let mut chr = ((b1 << 18) & 0x1C0000)
                        | ((b2 << 12) & 0x3F000)
                        | ((b3 << 6) & 0xFC0)
                        | (b4 & 0x3F);

                    if chr < 0x10000 || chr > 0x10FFFF {
                        result.extend([0xFFFD, 0xFFFD, 0xFFFD, 0xFFFD]);
                    } else {
                        chr -= 0x10000;
                        result.push((0xD800 + (chr >> 10)) as u16);
                        result.push((0xDC00 + (chr & 0x3FF)) as u16);
                    }

                    i += 9;
                    break 'body;
                }
            }

            result.push(0xFFFD);
        }
        i += 3;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str, exclude: Option<&str>) -> String {
        js::to_string(&decode(&js::to_utf16(s), exclude))
    }

    #[test]
    fn decode_matches_mdurl() {
        // values from node (tests/markdown_it/core_values.cjs --examples)
        // excluded characters stay encoded, re-cased to upper case
        assert_eq!(dec("%41%2f%2F%25%zz", None), "A%2F%2F%%zz");
        assert_eq!(dec("%41%2f%25", Some(";/?:@&=+$,#%")), "A%2F%25");
        assert_eq!(
            dec("%C3%A4%E2%82%AC%F0%9F%98%80", None),
            "\u{e4}\u{20ac}\u{1F600}"
        );
        // overlong, surrogate, past U+10FFFF, stray continuation, truncated
        assert_eq!(
            dec("%C3%28 %C0%80 %ED%A0%80 %F4%90%80%80 %80 %C3", None),
            "\u{fffd}( \u{fffd}\u{fffd} \u{fffd}\u{fffd}\u{fffd} \u{fffd}\u{fffd}\u{fffd}\u{fffd} \u{fffd} \u{fffd}"
        );
        assert_eq!(
            dec("%e2%82 %F0%9F%98 %FF%FE%FD", None),
            "\u{fffd}\u{fffd} \u{fffd}\u{fffd}\u{fffd} \u{fffd}\u{fffd}\u{fffd}"
        );
        // an exclude naming an Object.prototype member: `undefined` for ASCII bytes
        assert_eq!(dec("%41%C3%A4", Some("valueOf")), "undefined\u{e4}");
        assert_eq!(dec("%%41%4%", None), "%A%4%");
    }
}
