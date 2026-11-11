//! Port of rules_inline/link.ts: Process `[link](<to> "stuff")`

use super::state_inline::StateInline;
use crate::markdown_it::common::utils::{is_space, normalize_reference};
use crate::markdown_it::helpers::{parse_link_destination, parse_link_label, parse_link_title};
use crate::markdown_it::js;
use crate::markdown_it::token::AttrValue;

// link.ts's function is split in four, in its own order: `link` keeps the steps that recurse
// (the two label scans, the label's tokenizing) and calls the others, which hold the big
// temporaries, so that a level of nested links or brackets costs little stack (the engine's
// recursion limit, `MAX_DEPTH`, is measured on it). Nothing else changes.

/// What `link` found after the label: `pos`, `href`, `title`, and whether the inline form
/// failed and the reference form must be tried (`parseReference`).
pub(super) struct Destination {
    pub pos: usize,
    pub href: Vec<u16>,
    pub title: Vec<u16>,
    pub parse_reference: bool,
}

pub fn link(state: &mut StateInline, silent: bool) -> bool {
    if js::char_code_at(&state.src, state.pos) != Some(0x5B /* [ */) {
        return false;
    }
    // Not JS's: a level of node's stack for the label's scan and tokenizing (see `cost`);
    // past it the RangeError is thrown.
    let Some(_level) = state.env.level(crate::markdown_it::cost::LINK) else {
        return false;
    };
    // The label's scan and tokenizing recurse here: on a stack with room for them.
    crate::depth::grow!(link_here(state, silent))
}

/// The rest of [`link`], from its label's scan on.
#[inline(always)]
fn link_here(state: &mut StateInline, silent: bool) -> bool {
    let old_pos = state.pos;
    let max = state.pos_max;
    let label_start = state.pos + 1;
    let label_end = parse_link_label(state, state.pos, true);

    // parser failed to find ']', so it's not a valid link
    let Some(label_end) = label_end else {
        return false;
    };

    let Some(mut dest) = inline_destination(state, label_end, max) else {
        return false;
    };

    // JS `label`: `undefined` until the reference branch sets it
    let mut label: Option<String> = None;
    if dest.parse_reference {
        //
        // Link reference
        //
        // `typeof state.env.references === 'undefined'`
        if state.env.references.is_none() {
            return false;
        }

        let mut raw_label: Option<(usize, usize)> = None;
        let mut pos = dest.pos;
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
        dest.pos = pos;

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
        state.pos = label_start;
        state.pos_max = label_end;

        push_link_open(state, &dest, label.as_deref());

        state.link_level += 1;
        let md = state.md;
        md.inline.tokenize(state);
        state.link_level -= 1;

        state.push("link_close", "a", -1);
    }

    state.pos = dest.pos;
    state.pos_max = max;
    true
}

/// The inline form after the label (`(  <href>  "title"  )`), up to deciding whether the
/// reference form must be tried; `None` where link.ts returns false.
#[inline(never)]
fn inline_destination(
    state: &mut StateInline,
    label_end: usize,
    max: usize,
) -> Option<Destination> {
    let mut href: Vec<u16> = Vec::new();
    let mut title: Vec<u16> = Vec::new();
    let mut parse_reference = true;

    let mut pos = label_end + 1;
    if pos < max && js::char_code_at(&state.src, pos) == Some(0x28 /* ( */) {
        //
        // Inline link
        //

        // might have found a valid shortcut link, disable reference parsing
        parse_reference = false;

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
            return None;
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
            }
        }

        if pos >= max || js::char_code_at(&state.src, pos) != Some(0x29 /* ) */) {
            // parsing a valid shortcut link failed, fallback to reference
            parse_reference = true;
        }
        pos += 1;
    }

    Some(Destination {
        pos,
        href,
        title,
        parse_reference,
    })
}

/// The reference lookup: the label (`raw_label`'s span, or the link text when it is absent
/// or empty) normalized, its `href` and `title` into `dest`; `None` where there is no such
/// reference. Returns the normalized label (JS's `label`).
#[inline(never)]
pub(super) fn reference(
    state: &StateInline,
    raw_label: Option<(usize, usize)>,
    label_start: usize,
    label_end: usize,
    dest: &mut Destination,
) -> Option<String> {
    // covers label === '' and label === undefined
    // (collapsed reference link and shortcut reference link respectively)
    let raw_label = match raw_label {
        Some((start, end)) if start < end && start < state.src.len() => {
            js::slice_to(&state.src, start, end)
        }
        _ => js::slice_to(&state.src, label_start, label_end),
    };

    let normalized = normalize_reference(raw_label);
    let reference = state
        .env
        .references
        .as_ref()
        .and_then(|references| references.get(&normalized))?;
    dest.href = js::to_utf16(&reference.href);
    dest.title = js::to_utf16(&reference.title);
    Some(normalized)
}

/// `link_open` with its `href`, `title` and `meta.label`.
#[inline(never)]
fn push_link_open(state: &mut StateInline, dest: &Destination, label: Option<&str>) {
    let token_o = state.push("link_open", "a", 1);
    let mut attrs = vec![("href".to_owned(), AttrValue::Str(js::to_string(&dest.href)))];
    if !dest.title.is_empty() {
        attrs.push((
            "title".to_owned(),
            AttrValue::Str(js::to_string(&dest.title)),
        ));
    }
    token_o.attrs = Some(attrs);
    if let Some(label) = label.filter(|l| !l.is_empty()) {
        token_o.meta = Some(serde_json::json!({ "label": label }));
    }
}

/// Helpers for the unit tests of link, image, emphasis and strikethrough. Each test's
/// expected values come from the markdown-it bundled in upstream Markdoc 0.5.11, through the
/// node calls its comment names; `show` is the compact token form those calls printed.
#[cfg(test)]
pub(super) mod test_support {
    use std::collections::HashMap;

    use crate::markdown_it::rules_inline::state_inline::StateInline;
    use crate::markdown_it::token::Token;
    use crate::markdown_it::{Env, MarkdownIt, Reference};

    /// `kind(tag)[attrs]{meta.label}@level"content"/markup^nesting <children>`, empty parts left
    /// out (the same function ran in node).
    pub fn show(t: &Token) -> String {
        let mut s = t.kind.clone();
        if !t.tag.is_empty() {
            s += &format!("({})", t.tag);
        }
        if let Some(attrs) = &t.attrs {
            let attrs: Vec<String> = attrs
                .iter()
                .map(|(k, v)| format!("{k}={}", v.to_js_string()))
                .collect();
            s += &format!("[{}]", attrs.join(","));
        }
        if let Some(meta) = &t.meta {
            s += &format!("{{{}}}", meta["label"].as_str().unwrap_or_default());
        }
        s += &format!("@{}", t.level);
        if !t.content.is_empty() {
            s += &serde_json::to_string(&t.content).unwrap();
        }
        if !t.markup.is_empty() {
            s += &format!("/{}", t.markup);
        }
        if t.nesting != 0 {
            s += &format!("^{}", t.nesting);
        }
        if let Some(children) = t.children.as_ref().filter(|c| !c.is_empty()) {
            s += &format!(" <{}>", show_all(children).join(" "));
        }
        s
    }

    pub fn show_all(tokens: &[Token]) -> Vec<String> {
        tokens.iter().map(show).collect()
    }

    /// `state.delimiters` as `marker:length:token:end:oc` (open, close).
    pub fn delimiters_of(state: &StateInline) -> Vec<String> {
        let Some(delimiters) = state.delimiters.as_ref() else {
            return vec!["undefined".to_owned()];
        };
        delimiters
            .borrow()
            .iter()
            .map(|d| {
                let open = if d.open { "o" } else { "-" };
                let close = if d.close { "c" } else { "-" };
                format!(
                    "{}:{}:{}:{}:{open}{close}",
                    d.marker, d.length, d.token, d.end
                )
            })
            .collect()
    }

    /// node: `md.core.ruler.enableOnly(['normalize', 'block', 'inline'])`,
    /// `md.inline.ruler.enableOnly(['link', 'image'])`, `md.inline.ruler2.enableOnly([])`, so
    /// that the expectations do not depend on rules other files port.
    pub fn links_only(preset: &str) -> MarkdownIt {
        let mut md = MarkdownIt::new(preset).unwrap();
        md.core
            .ruler
            .enable_only(&["normalize", "block", "inline"], false)
            .unwrap();
        md.inline
            .ruler
            .enable_only(&["link", "image"], false)
            .unwrap();
        md.inline.ruler2.enable_only::<&str>(&[], false).unwrap();
        md
    }

    /// `{FOO: {href: '/f', title: 'T'}, BAR: {href: '/b', title: ''}, 'A B': {href: '', title: ''}}`
    pub fn refs_env() -> Env {
        let r = |href: &str, title: &str| Reference {
            href: href.to_owned(),
            title: title.to_owned(),
        };
        Env {
            references: Some(HashMap::from([
                ("FOO".to_owned(), r("/f", "T")),
                ("BAR".to_owned(), r("/b", "")),
                ("A B".to_owned(), r("", "")),
            ])),
            ..Env::default()
        }
    }

    /// `md.parseInline(src, env)[0].children`, shown.
    pub fn inline(md: &MarkdownIt, src: &str, env: &mut Env) -> Vec<String> {
        let tokens = md.parse_inline(src, env);
        show_all(tokens[0].children.as_ref().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{inline, links_only, refs_env};
    use super::*;
    use crate::markdown_it::Env;

    #[test]
    fn inline_links_match_node() {
        // node (unit_values.cjs `inl`): links_only md; parseInline(src, {})[0].children
        let md = links_only("default");
        let cases: [(&str, &[&str]); 11] = [
            (
                "[a](/u \"t\") x",
                &[
                    "link_open(a)[href=/u,title=t]@0^1",
                    "text@1\"a\"",
                    "link_close(a)@0^-1",
                    "text@0\" x\"",
                ],
            ),
            (
                "[a [b] c](/u)",
                &[
                    "link_open(a)[href=/u]@0^1",
                    "text@1\"a [b] c\"",
                    "link_close(a)@0^-1",
                ],
            ),
            // no links in links: the outer label is refused (disableNested)
            (
                "[a [b](/x) c](/y)",
                &[
                    "text@0\"[a \"",
                    "link_open(a)[href=/x]@0^1",
                    "text@1\"b\"",
                    "link_close(a)@0^-1",
                    "text@0\" c](/y)\"",
                ],
            ),
            (
                "[[a](/x)](/y)",
                &[
                    "text@0\"[\"",
                    "link_open(a)[href=/x]@0^1",
                    "text@1\"a\"",
                    "link_close(a)@0^-1",
                    "text@0\"](/y)\"",
                ],
            ),
            // validateLink refuses it: pos stays before the URL, `)` is not found, and with
            // no env.references the reference fallback gives up
            ("[a](javascript:x)", &["text@0\"[a](javascript:x)\""]),
            (
                "[a](  /u  't'  )",
                &[
                    "link_open(a)[href=/u,title=t]@0^1",
                    "text@1\"a\"",
                    "link_close(a)@0^-1",
                ],
            ),
            (
                "[a](\n/u\n)",
                &[
                    "link_open(a)[href=/u]@0^1",
                    "text@1\"a\"",
                    "link_close(a)@0^-1",
                ],
            ),
            (
                "[a]()",
                &[
                    "link_open(a)[href=]@0^1",
                    "text@1\"a\"",
                    "link_close(a)@0^-1",
                ],
            ),
            ("[a](/u", &["text@0\"[a](/u\""]),
            ("[foo](", &["text@0\"[foo](\""]),
            ("[nope]", &["text@0\"[nope]\""]),
        ];
        for (src, want) in cases {
            assert_eq!(inline(&md, src, &mut Env::default()), want, "{src}");
        }
    }

    #[test]
    fn reference_links_match_node() {
        // node (unit_values.cjs `ref`): parseInline(src, {references: REFS}); without the
        // references every one of these stays text (`typeof env.references === 'undefined'`)
        let md = links_only("default");
        let foo = [
            "link_open(a)[href=/f,title=T]{FOO}@0^1",
            "text@1\"foo\"",
            "link_close(a)@0^-1",
        ];
        let cases: [(&str, Vec<&str>); 11] = [
            ("[foo]", foo.to_vec()),
            (
                "[x][foo]",
                vec![
                    "link_open(a)[href=/f,title=T]{FOO}@0^1",
                    "text@1\"x\"",
                    "link_close(a)@0^-1",
                ],
            ),
            (
                "[Foo][]",
                vec![
                    "link_open(a)[href=/f,title=T]{FOO}@0^1",
                    "text@1\"Foo\"",
                    "link_close(a)@0^-1",
                ],
            ),
            ("[foo][]x", [&foo[..], &["text@0\"x\""]].concat()),
            ("[x][nope]", vec!["text@0\"[x][nope]\""]),
            ("[x][]", vec!["text@0\"[x][]\""]),
            // an empty href and title still make a link; the label is normalized
            (
                "[a   b]",
                vec![
                    "link_open(a)[href=]{A B}@0^1",
                    "text@1\"a   b\"",
                    "link_close(a)@0^-1",
                ],
            ),
            // `[bar` is no label: shortcut reference `[foo]`, the rest is text
            ("[foo][bar", [&foo[..], &["text@0\"[bar\""]].concat()),
            // an inline link wins over the reference
            (
                "[foo](/u) [bar]",
                vec![
                    "link_open(a)[href=/u]@0^1",
                    "text@1\"foo\"",
                    "link_close(a)@0^-1",
                    "text@0\" \"",
                    "link_open(a)[href=/b]{BAR}@0^1",
                    "text@1\"bar\"",
                    "link_close(a)@0^-1",
                ],
            ),
            // a failed inline link `(` falls back to the reference
            ("[foo](", vec!["text@0\"[foo](\""]),
            (
                "[bar] y",
                vec![
                    "link_open(a)[href=/b]{BAR}@0^1",
                    "text@1\"bar\"",
                    "link_close(a)@0^-1",
                    "text@0\" y\"",
                ],
            ),
        ];
        for (src, want) in cases {
            assert_eq!(inline(&md, src, &mut refs_env()), want, "{src}");
            let plain = format!("text@0{}", serde_json::to_string(src).unwrap());
            if !src.contains("](/u)") {
                assert_eq!(inline(&md, src, &mut Env::default()), [plain], "{src}");
            }
        }
    }

    fn probe(state: &mut StateInline, silent: bool) -> bool {
        if js::char_code_at(&state.src, state.pos) != Some(0x40 /* @ */) {
            return false;
        }
        if !silent {
            let link_level = state.link_level;
            let t = state.push("probe", "", 0);
            t.content = link_level.to_string();
        }
        state.pos += 1;
        true
    }

    #[test]
    fn link_level_is_raised_while_the_label_is_tokenized() {
        // node (unit_values.cjs `link_level`): a rule before 'link' that pushes a token holding
        // state.linkLevel at each '@'. Image alt text is parsed with a fresh state (level 0).
        let mut md = links_only("default");
        md.inline.ruler.before("link", "probe", probe, &[]).unwrap();
        assert_eq!(
            inline(&md, "@[@[@](/b)](/a) @![@](/i)", &mut Env::default()),
            [
                "probe@0\"0\"",
                "text@0\"[\"",
                "probe@0\"0\"",
                "link_open(a)[href=/b]@0^1",
                "probe@1\"1\"",
                "link_close(a)@0^-1",
                "text@0\"](/a) \"",
                "probe@0\"0\"",
                "image(img)[src=/i,alt=]@0\"@\" <probe@0\"0\">",
            ]
        );
    }

    #[test]
    fn silent_mode_only_moves_pos() {
        // node (unit_values.cjs `link silent`, `link nope pos`): the rule called directly
        let md = links_only("default");
        let mut env = Env::default();
        let mut st = StateInline::new(js::to_utf16("[a](/u) x"), &md, &mut env, Vec::new());
        assert!(link(&mut st, true));
        assert_eq!(
            (st.pos, st.pos_max, st.tokens.len(), st.link_level),
            (7, 9, 0, 0)
        );

        let mut env = Env {
            references: Some(Default::default()),
            ..Env::default()
        };
        let mut st = StateInline::new(js::to_utf16("x [nope] y"), &md, &mut env, Vec::new());
        st.pos = 2;
        assert!(!link(&mut st, false));
        assert_eq!((st.pos, st.pos_max, st.tokens.len()), (2, 10, 0));
    }

    #[test]
    fn max_nesting_stops_label_scanning() {
        // node (unit_values.cjs `maxNesting`): commonmark (maxNesting 20), 21 nested labels
        let md = links_only("commonmark");
        let src = format!("{}a{}(/u)", "[".repeat(21), "]".repeat(21));
        let want = format!("text@0{}", serde_json::to_string(&src).unwrap());
        assert_eq!(inline(&md, &src, &mut Env::default()), [want]);
    }
}
