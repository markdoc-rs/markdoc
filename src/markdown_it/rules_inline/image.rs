//! Port of rules_inline/image.ts: Process ![image](<src> "title")

use super::link::{Destination, reference};
use super::state_inline::StateInline;
use crate::markdown_it::common::utils::is_space;
use crate::markdown_it::helpers::{parse_link_destination, parse_link_label, parse_link_title};
use crate::markdown_it::js;
use crate::markdown_it::token::{AttrValue, Token};

// image.ts's function is split as link.ts's is (see there): `image` keeps the steps that
// recurse (the label scans, the description's parse) and calls the others, in its own order.

pub fn image(state: &mut StateInline, silent: bool) -> bool {
    if js::char_code_at(&state.src, state.pos) != Some(0x21 /* ! */) {
        return false;
    }
    if js::char_code_at(&state.src, state.pos + 1) != Some(0x5B /* [ */) {
        return false;
    }
    // Not JS's: a level of node's stack for the description's scan and parse (see `cost`);
    // past it the RangeError is thrown.
    let Some(_level) = state.env.level(crate::markdown_it::cost::IMAGE) else {
        return false;
    };
    // The description's scan and parse recurse here: on a stack with room for them.
    crate::depth::grow!(image_here(state, silent))
}

/// The rest of [`image`], from its description's scan on.
#[inline(always)]
fn image_here(state: &mut StateInline, silent: bool) -> bool {
    let old_pos = state.pos;
    let max = state.pos_max;
    let label_start = state.pos + 2;
    let label_end = parse_link_label(state, state.pos + 1, false);

    // parser failed to find ']', so it's not a valid link
    let Some(label_end) = label_end else {
        return false;
    };

    let mut label: Option<String> = None;
    let mut dest;
    let pos = label_end + 1;
    if pos < max && js::char_code_at(&state.src, pos) == Some(0x28 /* ( */) {
        //
        // Inline link
        //
        match inline_destination(state, pos, max) {
            InlineDestination::Found(found) => dest = found,
            InlineDestination::NotFound => return false,
            InlineDestination::Unclosed => {
                state.pos = old_pos;
                return false;
            }
        }
    } else {
        //
        // Link reference
        //
        // `typeof state.env.references === 'undefined'`
        if state.env.references.is_none() {
            return false;
        }

        let mut raw_label: Option<(usize, usize)> = None;
        let mut pos = pos;
        if pos < max && js::char_code_at(&state.src, pos) == Some(0x5B /* [ */) {
            let start = pos + 1;
            // `pos = parseLinkLabel(state, pos)`, which is -1 (`None`) when there is no label
            match parse_link_label(state, pos, false) {
                Some(end) => {
                    // label = state.src.slice(start, pos++)
                    raw_label = Some((start, end));
                    pos = end + 1;
                }
                None => pos = label_end + 1,
            }
        } else {
            pos = label_end + 1;
        }

        dest = Destination {
            pos,
            href: Vec::new(),
            title: Vec::new(),
            parse_reference: true,
        };
        match reference(state, raw_label, label_start, label_end, &mut dest) {
            Some(normalized) => label = Some(normalized),
            None => {
                state.pos = old_pos;
                return false;
            }
        }
    }

    //
    // We found the end of the link, and know for a fact it's a valid link;
    // so all that's left to do is to call tokenizer.
    //
    if !silent {
        let content = js::slice_to(&state.src, label_start, label_end).to_vec();

        let mut tokens: Vec<Token> = Vec::new();
        let md = state.md;
        md.inline.parse(&content, md, state.env, &mut tokens);

        push_image(state, content, tokens, &dest, label.as_deref());
    }

    state.pos = dest.pos;
    state.pos_max = max;
    true
}

/// How the inline form (`(  <href>  "title"  )`, `pos` at its `(`) ended.
enum InlineDestination {
    Found(Destination),
    /// Nothing but spaces after `(` (image.ts returns false).
    NotFound,
    /// No `)` where it must be (image.ts restores `state.pos` and returns false).
    Unclosed,
}

#[inline(never)]
fn inline_destination(state: &mut StateInline, mut pos: usize, max: usize) -> InlineDestination {
    let mut href: Vec<u16> = Vec::new();
    let title: Vec<u16>;

    // [link](  <href>  "title"  )
    //        ^^ skipping these spaces
    pos += 1;
    while pos < max {
        let code = js::char_code_at(&state.src, pos);
        if !code.is_some_and(is_space) && code != Some(0x0A) {
            break;
        }
        pos += 1;
    }
    if pos >= max {
        return InlineDestination::NotFound;
    }

    // [link](  <href>  "title"  )
    //          ^^^^^^ parsing link destination
    // (JS also sets `start = pos` here, a value nothing reads.)
    let res = parse_link_destination(&state.src, pos, state.pos_max);
    if res.ok {
        href = state.md.normalize_link(&res.str);
        if state.md.validate_link(&href) {
            pos = res.pos;
        } else {
            href = Vec::new();
        }
    }

    // [link](  <href>  "title"  )
    //                ^^ skipping these spaces
    let start = pos;
    while pos < max {
        let code = js::char_code_at(&state.src, pos);
        if !code.is_some_and(is_space) && code != Some(0x0A) {
            break;
        }
        pos += 1;
    }

    // [link](  <href>  "title"  )
    //                  ^^^^^^^ parsing link title
    let res = parse_link_title(&state.src, pos, state.pos_max, None);
    if pos < max && start != pos && res.ok {
        title = res.str;
        pos = res.pos;

        // [link](  <href>  "title"  )
        //                         ^^ skipping these spaces
        while pos < max {
            let code = js::char_code_at(&state.src, pos);
            if !code.is_some_and(is_space) && code != Some(0x0A) {
                break;
            }
            pos += 1;
        }
    } else {
        title = Vec::new();
    }

    if pos >= max || js::char_code_at(&state.src, pos) != Some(0x29 /* ) */) {
        return InlineDestination::Unclosed;
    }
    pos += 1;
    InlineDestination::Found(Destination {
        pos,
        href,
        title,
        parse_reference: false,
    })
}

/// The `image` token, with the description's tokens as its children.
#[inline(never)]
fn push_image(
    state: &mut StateInline,
    content: Vec<u16>,
    tokens: Vec<Token>,
    dest: &Destination,
    label: Option<&str>,
) {
    let token = state.push("image", "img", 0);
    let mut attrs = vec![
        ("src".to_owned(), AttrValue::Str(js::to_string(&dest.href))),
        ("alt".to_owned(), AttrValue::Str(String::new())),
    ];
    token.children = Some(tokens);
    token.content = js::to_string(&content);

    if !dest.title.is_empty() {
        attrs.push((
            "title".to_owned(),
            AttrValue::Str(js::to_string(&dest.title)),
        ));
    }
    token.attrs = Some(attrs);
    if let Some(label) = label.filter(|l| !l.is_empty()) {
        token.meta = Some(serde_json::json!({ "label": label }));
    }
}

#[cfg(test)]
mod tests {
    use super::super::link::test_support::{inline, links_only, refs_env};
    use super::*;
    use crate::markdown_it::Env;

    #[test]
    fn inline_images_match_node() {
        // node (unit_values.cjs `inl`): links_only md; parseInline(src, {})[0].children
        let md = links_only("default");
        let cases: [(&str, &[&str]); 7] = [
            (
                "![a](/i \"t\")",
                &["image(img)[src=/i,alt=,title=t]@0\"a\" <text@0\"a\">"],
            ),
            // images nest; the alt text is parsed with a fresh state
            (
                "![a ![b](/c) d](/e)",
                &[
                    "image(img)[src=/e,alt=]@0\"a ![b](/c) d\" <text@0\"a \" image(img)[src=/c,alt=]@0\"b\" <text@0\"b\"> text@0\" d\">",
                ],
            ),
            // links may sit in image alt text
            (
                "![a [b](/x) c](/y)",
                &[
                    "image(img)[src=/y,alt=]@0\"a [b](/x) c\" <text@0\"a \" link_open(a)[href=/x]@0^1 text@1\"b\" link_close(a)@0^-1 text@0\" c\">",
                ],
            ),
            ("![](/u)", &["image(img)[src=/u,alt=]@0"]),
            ("![a](/u", &["text@0\"![a](/u\""]),
            ("![a](javascript:x)", &["text@0\"![a](javascript:x)\""]),
            ("![foo]", &["text@0\"![foo]\""]),
        ];
        for (src, want) in cases {
            assert_eq!(inline(&md, src, &mut Env::default()), want, "{src}");
        }
    }

    #[test]
    fn reference_images_match_node() {
        // node (unit_values.cjs `ref`): parseInline(src, {references: REFS})
        let md = links_only("default");
        let cases: [(&str, &str); 4] = [
            (
                "![foo]",
                "image(img)[src=/f,alt=,title=T]{FOO}@0\"foo\" <text@0\"foo\">",
            ),
            (
                "![x][Bar]",
                "image(img)[src=/b,alt=]{BAR}@0\"x\" <text@0\"x\">",
            ),
            (
                "![foo][]",
                "image(img)[src=/f,alt=,title=T]{FOO}@0\"foo\" <text@0\"foo\">",
            ),
            ("![nope]", "text@0\"![nope]\""),
        ];
        for (src, want) in cases {
            assert_eq!(inline(&md, src, &mut refs_env()), [want], "{src}");
        }
    }

    #[test]
    fn silent_mode_only_moves_pos() {
        // node (unit_values.cjs `image silent`): the rule called directly
        let md = links_only("default");
        let mut env = Env::default();
        let mut st = StateInline::new(js::to_utf16("![a](/u) x"), &md, &mut env, Vec::new());
        assert!(image(&mut st, true));
        assert_eq!((st.pos, st.pos_max, st.tokens.len()), (8, 10, 0));
    }
}
