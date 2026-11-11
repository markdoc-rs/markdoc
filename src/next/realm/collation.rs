//! `String.prototype.localeCompare` for ASCII text: ICU's root collation (node's default
//! locale collates as the root), from the weights the table generator takes from node.

use super::table::{ASCII_PRIMARY, ASCII_TERTIARY};

/// `a.localeCompare(b)` for two ASCII strings: -1, 0 or 1. The primary weights of the
/// characters that are not ignorable compare first, then their tertiary weights (lower case
/// before upper case); ignorable characters (most C0 controls) count for nothing.
pub(super) fn compare_ascii(a: &[u16], b: &[u16]) -> f64 {
    let weights = |s: &[u16], table: &[u8; 128]| -> Vec<u8> {
        s.iter()
            .filter(|&&c| ASCII_PRIMARY[c as usize] != 0)
            .map(|&c| table[c as usize])
            .collect()
    };
    let order = weights(a, &ASCII_PRIMARY)
        .cmp(&weights(b, &ASCII_PRIMARY))
        .then_with(|| weights(a, &ASCII_TERTIARY).cmp(&weights(b, &ASCII_TERTIARY)));
    match order {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    }
}
