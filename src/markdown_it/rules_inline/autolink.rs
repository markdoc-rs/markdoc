//! Port of rules_inline/autolink.ts.
//!
//! Process autolinks '<protocol:...>'

use super::state_inline::StateInline;
use crate::markdown_it::js;

fn is_ascii_alpha(c: u16) -> bool {
    (0x41..=0x5A).contains(&c) || (0x61..=0x7A).contains(&c)
}

fn is_ascii_alnum(c: u16) -> bool {
    is_ascii_alpha(c) || (0x30..=0x39).contains(&c)
}

/* eslint max-len:0 */
// const EMAIL_RE = /^([a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*)$/
//
// Hand-written over UTF-16 code units (every class is ASCII, so a non-ASCII code unit never
// matches). No backtracking can change the outcome: the local part's class has no `@`, so it
// is the whole run before the first `@`; a domain label's class has no `.`, so the labels
// are the `.`-separated parts of the rest, each of which must match
// `[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?` entirely: 1 to 63 of `[a-zA-Z0-9-]`, with
// no `-` first or last.
fn email_re_test(s: &[u16]) -> bool {
    fn is_local(c: u16) -> bool {
        is_ascii_alnum(c) || (c < 0x80 && b".!#$%&'*+/=?^_`{|}~-".contains(&(c as u8)))
    }
    fn is_label(label: &[u16]) -> bool {
        (1..=63).contains(&label.len())
            && label
                .iter()
                .all(|&c| is_ascii_alnum(c) || c == 0x2D /* - */)
            && is_ascii_alnum(label[0])
            && is_ascii_alnum(label[label.len() - 1])
    }

    let local = s.iter().take_while(|&&c| is_local(c)).count();
    if local == 0 || s.get(local) != Some(&0x40 /* @ */) {
        return false;
    }
    s[local + 1..].split(|&c| c == 0x2E /* . */).all(is_label)
}

/* eslint-disable-next-line no-control-regex */
// const AUTOLINK_RE = /^([a-zA-Z][a-zA-Z0-9+.-]{1,31}):([^<>\x00-\x20]*)$/
//
// Hand-written over UTF-16 code units. The scheme's class has no `:`, so the scheme is the
// whole run of `[a-zA-Z0-9+.-]` after the first letter (backtracking to a shorter run would
// leave a scheme character where `:` must be): 1 to 31 of them, then `:`. The rest must
// consist of code units other than `<`, `>` and U+0000-U+0020 (a negated class matches any
// other code unit, lone surrogates included).
fn autolink_re_test(s: &[u16]) -> bool {
    fn is_scheme(c: u16) -> bool {
        is_ascii_alnum(c) || c == 0x2B /* + */ || c == 0x2E /* . */ || c == 0x2D /* - */
    }

    if !s.first().is_some_and(|&c| is_ascii_alpha(c)) {
        return false;
    }
    let scheme = s[1..].iter().take_while(|&&c| is_scheme(c)).count();
    if !(1..=31).contains(&scheme) || s.get(1 + scheme) != Some(&0x3A /* : */) {
        return false;
    }
    s[2 + scheme..]
        .iter()
        .all(|&c| c != 0x3C /* < */ && c != 0x3E /* > */ && c > 0x20)
}

pub fn autolink(state: &mut StateInline, silent: bool) -> bool {
    let mut pos = state.pos;

    if js::char_code_at(&state.src, pos) != Some(0x3C /* < */) {
        return false;
    }

    let start = state.pos;
    let max = state.pos_max;

    loop {
        pos += 1;
        if pos >= max {
            return false;
        }

        let ch = js::char_code_at(&state.src, pos);

        if ch == Some(0x3C /* < */) {
            return false;
        }
        if ch == Some(0x3E /* > */) {
            break;
        }
    }

    let url = js::slice_to(&state.src, start + 1, pos).to_vec();

    if autolink_re_test(&url) {
        let full_url = state.md.normalize_link(&url);
        if !state.md.validate_link(&full_url) {
            return false;
        }

        if !silent {
            let text = state.md.normalize_link_text(&url);

            let token_o = state.push("link_open", "a", 1);
            token_o.attrs = Some(vec![("href".into(), js::to_string(&full_url).into())]);
            token_o.markup = "autolink".into();
            token_o.info = "auto".into();

            let token_t = state.push("text", "", 0);
            token_t.set_content_utf16(&text);

            let token_c = state.push("link_close", "a", -1);
            token_c.markup = "autolink".into();
            token_c.info = "auto".into();
        }

        state.pos += url.len() + 2;
        return true;
    }

    if email_re_test(&url) {
        // `mailto:${url}`
        let mut mailto = js::to_utf16("mailto:");
        mailto.extend_from_slice(&url);
        let full_url = state.md.normalize_link(&mailto);
        if !state.md.validate_link(&full_url) {
            return false;
        }

        if !silent {
            let text = state.md.normalize_link_text(&url);

            let token_o = state.push("link_open", "a", 1);
            token_o.attrs = Some(vec![("href".into(), js::to_string(&full_url).into())]);
            token_o.markup = "autolink".into();
            token_o.info = "auto".into();

            let token_t = state.push("text", "", 0);
            token_t.set_content_utf16(&text);

            let token_c = state.push("link_close", "a", -1);
            token_c.markup = "autolink".into();
            token_c.info = "auto".into();
        }

        state.pos += url.len() + 2;
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autolink_re_matches_the_regex() {
        // node (tests/markdown_it/inline_rule_values.cjs, `autolink_re`): AUTOLINK_RE.test(s),
        // with the regex taken from the bundle
        let a32 = format!("{}:x", "a".repeat(32));
        let a33 = format!("{}:x", "a".repeat(33));
        let cases: [(&str, bool); 12] = [
            ("a:b", false),
            ("ab:c", true),
            (&a32, true),
            (&a33, false),
            ("1a:x", false),
            ("a+b.c-d:x", true),
            ("ab:", true),
            ("http://a b", false),
            ("http://a\u{A0}b", true),
            ("http://a\u{1}", false),
            ("http://a\u{7F}", true),
            ("ab:<", false),
        ];
        for (s, want) in cases {
            assert_eq!(autolink_re_test(&js::to_utf16(s)), want, "{s:?}");
        }
        // 'ab:\uD800': a lone surrogate is not in the excluded set
        assert!(autolink_re_test(&[0x61, 0x62, 0x3A, 0xD800]));
    }

    #[test]
    fn email_re_matches_the_regex() {
        // node (`email_re`): EMAIL_RE.test(s), with the regex taken from the bundle
        let l63 = format!("x@{}", "a".repeat(63));
        let l64 = format!("x@{}", "a".repeat(64));
        let hyphens = format!("x@a{}b", "-".repeat(61));
        let cases: [(&str, bool); 17] = [
            ("x@y", true),
            ("x@y.z", true),
            ("x@-y", false),
            ("x@y-", false),
            ("x@y..z", false),
            ("x@y.", false),
            ("x@.y", false),
            (&l63, true),
            (&l64, false),
            (&hyphens, true),
            ("!#$%&'*+/=?^_`{|}~-@e", true),
            ("@x", false),
            ("x@", false),
            ("x@y@z", false),
            ("\u{E9}@x", false),
            ("x@\u{E9}", false),
            ("x@a.b-c.d9", true),
        ];
        for (s, want) in cases {
            assert_eq!(email_re_test(&js::to_utf16(s)), want, "{s:?}");
        }
    }
}
