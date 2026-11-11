//! Not ECMAScript: what `Flags::v8_compat` needs that the specification does not say (see
//! markdoc's VENDORED.md).
//!
//! V8 compares a case-insensitive backreference with `u` or `v`, in a subject it stores
//! with two bytes a code unit (one holding a character past U+00FF), with ICU's
//! `UnicodeString::caseCompare(..., U_FOLD_CASE_DEFAULT)`: full case folding, so `/(ﬀf)\1/iu`
//! matches "ﬀffﬀ" (both halves fold to "fff"), which simple case folding, as ECMAScript
//! specifies, does not. A subject V8 stores with one byte a code unit is compared with
//! Latin-1 case pairs, which simple case folding gives as well.

mod full_folding;

use crate::unicode;
use full_folding::FULL_FOLDINGS;

/// ICU's `ucase_toFullFolding` (`U_FOLD_CASE_DEFAULT`): the full case folding of `c` as
/// UTF-16, or `None` where `c` folds to itself.
fn full_folding(c: u32) -> Option<([u16; 8], usize)> {
    let mut buf = [0u16; 8];
    let mut len = 0;
    let mut push = |c: u32| {
        if c > 0xFFFF {
            let d = c - 0x10000;
            buf[len] = 0xD800 + (d >> 10) as u16;
            buf[len + 1] = 0xDC00 + (d & 0x3FF) as u16;
            len += 2;
        } else {
            buf[len] = c as u16;
            len += 1;
        }
    };
    if let Ok(i) = FULL_FOLDINGS.binary_search_by(|(k, _)| k.cmp(&c)) {
        for &m in FULL_FOLDINGS[i].1 {
            push(m);
        }
        return Some((buf, len));
    }
    let folded = unicode::fold(c);
    if folded == c {
        return None;
    }
    push(folded);
    Some((buf, len))
}

fn is_lead(u: u16) -> bool {
    (0xD800..=0xDBFF).contains(&u)
}

fn is_trail(u: u16) -> bool {
    (0xDC00..=0xDFFF).contains(&u)
}

fn supplementary(lead: u16, trail: u16) -> u32 {
    0x10000 + ((u32::from(lead) - 0xD800) << 10) + (u32::from(trail) - 0xDC00)
}

/// One string as ICU's `_cmpFold` walks it: the string itself (level 0), or the full case
/// folding of one of its characters (level 1), which it reads before going back.
struct Side<'a> {
    text: &'a [u16],
    level: u8,
    /// Where level 0 resumes after the folding.
    saved: usize,
    fold: [u16; 8],
    fold_len: usize,
    /// Just past the last code unit read, in the current level's buffer.
    pos: usize,
}

impl<'a> Side<'a> {
    fn new(text: &'a [u16]) -> Self {
        Side {
            text,
            level: 0,
            saved: 0,
            fold: [0; 8],
            fold_len: 0,
            pos: 0,
        }
    }

    fn buf(&self) -> &[u16] {
        if self.level == 0 {
            self.text
        } else {
            &self.fold[..self.fold_len]
        }
    }

    /// The next code unit, popping a finished folding; `None` at the end.
    fn next_unit(&mut self) -> Option<u16> {
        loop {
            if self.pos < self.buf().len() {
                let c = self.buf()[self.pos];
                self.pos += 1;
                return Some(c);
            }
            if self.level == 0 {
                return None;
            }
            self.level = 0;
            self.pos = self.saved;
        }
    }

    /// The code point `c`, the unit just read, belongs to (its pair's, for a surrogate).
    fn code_point(&self, c: u16) -> u32 {
        let buf = self.buf();
        if is_lead(c) && self.pos < buf.len() && is_trail(buf[self.pos]) {
            return supplementary(c, buf[self.pos]);
        }
        if is_trail(c) && self.pos >= 2 && is_lead(buf[self.pos - 2]) {
            return supplementary(buf[self.pos - 2], c);
        }
        u32::from(c)
    }

    /// Goes on with the folding of the character just read.
    fn enter(&mut self, fold: ([u16; 8], usize)) {
        self.saved = self.pos;
        self.level = 1;
        self.fold = fold.0;
        self.fold_len = fold.1;
        self.pos = 0;
    }
}

/// ICU's `_cmpFold` (`ustrcase.cpp`) with `U_COMPARE_IGNORE_CASE` and `U_FOLD_CASE_DEFAULT`,
/// for equality: whether `a` and `b` are equal once case folded, as ICU decides it (a
/// character is folded only where the two strings' code units differ, one level deep).
pub(crate) fn case_compare_equal(a: &[u16], b: &[u16]) -> bool {
    let mut x = Side::new(a);
    let mut y = Side::new(b);
    let (mut c1, mut c2): (Option<u16>, Option<u16>) = (None, None);
    let (mut need1, mut need2) = (true, true);
    loop {
        if need1 {
            c1 = x.next_unit();
            need1 = false;
        }
        if need2 {
            c2 = y.next_unit();
            need2 = false;
        }
        let (u1, u2) = match (c1, c2) {
            (None, None) => return true,
            (Some(u1), Some(u2)) if u1 == u2 => {
                need1 = true;
                need2 = true;
                continue;
            }
            (Some(u1), Some(u2)) => (u1, u2),
            _ => return false,
        };
        let cp1 = x.code_point(u1);
        let cp2 = y.code_point(u2);
        if x.level == 0
            && let Some(fold) = full_folding(cp1)
        {
            if is_lead(u1) && cp1 > 0xFFFF {
                // Past the pair's trail too.
                x.pos += 1;
            } else if is_trail(u1) && cp1 > 0xFFFF {
                // The pair was entered at its trail: its lead matched the other string's,
                // which is compared with the folding instead.
                y.pos -= 1;
                c2 = y.buf().get(y.pos.wrapping_sub(1)).copied();
            }
            x.enter(fold);
            need1 = true;
            continue;
        }
        if y.level == 0
            && let Some(fold) = full_folding(cp2)
        {
            if is_lead(u2) && cp2 > 0xFFFF {
                y.pos += 1;
            } else if is_trail(u2) && cp2 > 0xFFFF {
                x.pos -= 1;
                c1 = x.buf().get(x.pos.wrapping_sub(1)).copied();
            }
            y.enter(fold);
            need2 = true;
            continue;
        }
        return false;
    }
}

#[cfg(test)]
mod tests {
    use super::case_compare_equal;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn compares_as_icu_case_compare() {
        // What V8 answers with these as a capture and the input after it, in a two-byte
        // subject (node 22: /(ﬀf)\1/iu.exec("ﬀffﬀ") matches).
        assert!(case_compare_equal(&u("\u{FB00}f"), &u("f\u{FB00}")));
        assert!(case_compare_equal(&u("\u{DF}s"), &u("s\u{DF}")));
        assert!(case_compare_equal(&u("\u{DF}"), &u("\u{1E9E}")));
        assert!(case_compare_equal(&u("K"), &u("\u{212A}")));
        assert!(case_compare_equal(&u("\u{1F80}"), &u("\u{1F88}")));
        assert!(!case_compare_equal(&u("\u{130}"), &u("i")));
        assert!(!case_compare_equal(&u("\u{DF}"), &u("s")));
        assert!(!case_compare_equal(&u("ab"), &u("ac")));
        assert!(case_compare_equal(&u("\u{10400}"), &u("\u{10428}")));
        assert!(case_compare_equal(&[], &[]));
    }
}
