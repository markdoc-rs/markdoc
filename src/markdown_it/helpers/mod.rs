//! Port of helpers/index.ts: functions used to parse links and images, split out of the
//! parser rules because of their size. (JS also exposes them as `md.helpers`; rules here
//! call these functions directly.)

pub mod parse_link_destination;
pub mod parse_link_label;
pub mod parse_link_title;

pub use parse_link_destination::{ParseLinkDestinationResult, parse_link_destination};
pub use parse_link_label::parse_link_label;
pub use parse_link_title::{ParseLinkTitleResult, parse_link_title};

#[cfg(test)]
#[allow(clippy::type_complexity)]
mod tests {
    use super::*;
    use crate::markdown_it::rules_inline::state_inline::StateInline;
    use crate::markdown_it::{Env, MarkdownIt, js};

    fn u(s: &str) -> Vec<u16> {
        js::to_utf16(s)
    }

    #[test]
    fn parse_link_label_matches_node() {
        // node, with md.inline.ruler.enableOnly([]) so skipToken steps one code unit:
        // parseLinkLabel(new md.inline.State(src, md, {}, []), start, disableNested)
        let mut md = MarkdownIt::default();
        md.inline.ruler.enable_only::<&str>(&[], false).unwrap();
        let cases: [(&str, usize, bool, Option<usize>, Option<usize>); 8] = [
            ("[foo]", 0, false, None, Some(4)),
            ("[a[b]c] x", 0, false, None, Some(6)),
            ("[a[b]c", 0, false, None, None),
            ("[a\\]b]", 0, false, None, Some(3)),
            ("[a[b]c]", 0, true, None, Some(6)),
            ("[foo]", 0, false, Some(3), None),
            ("x[y]", 1, false, None, Some(3)),
            ("[]", 0, false, None, Some(1)),
        ];
        for (src, start, disable_nested, pos_max, want) in cases {
            let mut env = Env::default();
            let mut st = StateInline::new(u(src), &md, &mut env, Vec::new());
            if let Some(m) = pos_max {
                st.pos_max = m;
            }
            assert_eq!(
                parse_link_label(&mut st, start, disable_nested),
                want,
                "{src}"
            );
            assert_eq!(st.pos, 0, "pos restored");
        }
    }

    #[test]
    fn parse_link_destination_matches_node() {
        // node: md.helpers.parseLinkDestination(str, start, max) -> {ok, pos, str}
        let deep_ok = format!("{}{}", "(".repeat(32), ")".repeat(32));
        let deep_bad = format!("{}{}", "(".repeat(33), ")".repeat(33));
        let cases: Vec<(&str, usize, usize, bool, usize, &str)> = vec![
            ("<foo bar>", 0, 9, true, 9, "foo bar"),
            ("<foo\nbar>", 0, 9, false, 0, ""),
            ("<a<b>", 0, 5, false, 0, ""),
            ("<a\\>b>", 0, 6, true, 6, "a>b"),
            ("foo(bar)baz qux", 0, 15, true, 11, "foo(bar)baz"),
            ("foo(bar", 0, 7, false, 0, ""),
            ("a\\ b c", 0, 6, true, 2, "a\\"),
            ("", 0, 0, false, 0, ""),
            (&deep_bad, 0, 66, false, 0, ""),
            (&deep_ok, 0, 64, true, 64, &deep_ok),
            ("&amp;x", 0, 6, true, 6, "&x"),
            ("a\u{1}b", 0, 3, true, 1, "a"),
            ("<>", 0, 2, true, 2, ""),
            ("<a", 0, 2, false, 0, ""),
            ("abc def", 0, 2, true, 2, "ab"),
            ("a)b", 0, 3, true, 1, "a"),
            ("<a\\", 0, 3, false, 0, ""),
            ("x<y", 1, 3, false, 0, ""),
        ];
        for (src, start, max, ok, pos, str) in cases {
            let r = parse_link_destination(&u(src), start, max);
            assert_eq!(
                (r.ok, r.pos, js::to_string(&r.str)),
                (ok, pos, str.to_owned()),
                "{src}"
            );
        }
    }

    #[test]
    fn parse_link_title_matches_node() {
        // node: md.helpers.parseLinkTitle(str, start, max[, prev]) -> {ok, can_continue, pos, str, marker}
        let cases: [(&str, usize, usize, (bool, bool, usize, &str, u16)); 9] = [
            ("\"title\" x", 0, 9, (true, false, 7, "title", 34)),
            ("'t\\'x'", 0, 6, (true, false, 6, "t'x", 39)),
            ("(a(b)", 0, 5, (false, false, 0, "", 41)),
            ("\"abc", 0, 4, (false, true, 0, "abc", 34)),
            ("x", 0, 1, (false, false, 0, "", 0)),
            ("", 0, 0, (false, false, 0, "", 0)),
            ("\"a\\\"b\"", 0, 6, (true, false, 6, "a\"b", 34)),
            ("(a)", 0, 3, (true, false, 3, "a", 41)),
            ("\"&amp;\"", 0, 7, (true, false, 7, "&", 34)),
        ];
        let tuple = |r: &ParseLinkTitleResult| {
            (r.ok, r.can_continue, r.pos, js::to_string(&r.str), r.marker)
        };
        for (src, start, max, (ok, cont, pos, str, marker)) in cases {
            let r = parse_link_title(&u(src), start, max, None);
            assert_eq!(tuple(&r), (ok, cont, pos, str.to_owned(), marker), "{src}");
        }
        // continuation on the next line (reference titles)
        let prev = parse_link_title(&u("\"ab\n"), 0, 4, None);
        assert_eq!(tuple(&prev), (false, true, 0, "ab\n".to_owned(), 34));
        let next = parse_link_title(&u("cd\" x"), 0, 5, Some(&prev));
        assert_eq!(tuple(&next), (true, false, 3, "ab\ncd".to_owned(), 34));
        let paren = parse_link_title(&u("(a\n"), 0, 3, None);
        let stop = parse_link_title(&u("(x"), 0, 2, Some(&paren));
        assert_eq!(tuple(&stop), (false, false, 0, "a\n".to_owned(), 41));
    }
}
