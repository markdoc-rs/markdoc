//! Port of rules_core/normalize.ts: normalize input string.

use super::state_core::StateCore;

/// `UNNORMALIZED_NEWLINE_RE = /\r\n?/g` becomes `\n` and `NULL_RE = /\0/g` becomes U+FFFD
/// (<https://spec.commonmark.org/0.29/#line-ending>). Both regexes are single code units
/// (CR, LF, NUL), matched by hand in two passes as the two `replace` calls do.
pub fn normalize(state: &mut StateCore) {
    let src = &state.src;

    // Normalize newlines
    let mut str: Vec<u16> = Vec::with_capacity(src.len());
    let mut i = 0;
    while i < src.len() {
        if src[i] == 0x0D {
            str.push(0x0A);
            i += if src.get(i + 1) == Some(&0x0A) { 2 } else { 1 };
            continue;
        }
        str.push(src[i]);
        i += 1;
    }

    // Replace NULL characters
    for c in str.iter_mut() {
        if *c == 0x00 {
            *c = 0xFFFD;
        }
    }

    state.src = str;
}
