//! Port of rules_inline/linkify.ts.
//!
//! Early process links like <https://example.org/> to have priority
//! over emphasis, etc.

use super::state_inline::StateInline;
use crate::markdown_it::js;

// RFC3986: scheme = ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )
fn is_ascii_alpha(code: u16) -> bool {
    (code >= 0x41 && code <= 0x5A) || (code >= 0x61 && code <= 0x7A)
}

fn is_scheme_char(code: u16) -> bool {
    (code >= 0x41 && code <= 0x5A)
        || (code >= 0x61 && code <= 0x7A)
        || (code >= 0x30 && code <= 0x39)
        || code == 0x2B
        || code == 0x2D
        || code == 0x2E
}

pub fn linkify(state: &mut StateInline, silent: bool) -> bool {
    if !state.md.options.linkify {
        return false;
    }
    if state.link_level > 0 {
        return false;
    }

    let pos = state.pos;
    let max = state.pos_max;

    if pos + 3 > max {
        return false;
    }
    if js::char_code_at(&state.src, pos) != Some(0x3A /* : */) {
        return false;
    }
    if js::char_code_at(&state.src, pos + 1) != Some(0x2F /* / */) {
        return false;
    }
    if js::char_code_at(&state.src, pos + 2) != Some(0x2F /* / */) {
        return false;
    }

    // Search backwards for the scheme, but no more than 10 characters, the length
    // of the pending string, or the length of the source prefix.
    // Use state.src instead of state.pending because it is faster to access.
    let proto_min = pos - 10.min(state.pending.len()).min(pos);
    let mut proto_start = pos;
    while proto_start > proto_min
        && js::char_code_at(&state.src, proto_start - 1).is_some_and(is_scheme_char)
    {
        proto_start -= 1;
    }

    if proto_start == pos || !js::char_code_at(&state.src, proto_start).is_some_and(is_ascii_alpha)
    {
        return false;
    }

    let proto_length = pos - proto_start;

    let link = match state.md.linkify.match_at_start(js::slice_to(
        &state.src,
        proto_start,
        state.src.len(),
    )) {
        Ok(Some(link)) => link,
        Ok(None) => return false,
        Err(error) => {
            state.env.throw(error);
            return false;
        }
    };

    let mut url = link.url;

    // invalid link, but still detected by linkify somehow;
    // need to check to prevent infinite loop below
    if url.len() <= proto_length {
        return false;
    }

    // disallow '*' at the end of the link (conflicts with emphasis)
    // do manual backsearch to avoid perf issues with regex /\*+$/ on "****...****a".
    let mut url_end = url.len();
    while url_end > 0 && url[url_end - 1] == u16::from(b'*') {
        url_end -= 1;
    }
    if url_end != url.len() {
        url.truncate(url_end);
    }

    let full_url = state.md.normalize_link(&url);
    if !state.md.validate_link(&full_url) {
        return false;
    }

    if !silent {
        // state.pending = state.pending.slice(0, -protoLength)
        state.pending = js::slice(&state.pending, 0, Some(-(proto_length as i64))).to_vec();

        let text = state.md.normalize_link_text(&url);

        let token_o = state.push("link_open", "a", 1);
        token_o.attrs = Some(vec![("href".into(), js::to_string(&full_url).into())]);
        token_o.markup = "linkify".into();
        token_o.info = "auto".into();

        let token_t = state.push("text", "", 0);
        token_t.set_content_utf16(&text);

        let token_c = state.push("link_close", "a", -1);
        token_c.markup = "linkify".into();
        token_c.info = "auto".into();
    }

    // state.pos += url.length - protoLength (in i64, as JS numbers can go negative)
    state.pos = (state.pos as i64 + url.len() as i64 - proto_length as i64) as usize;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown_it::{Env, MarkdownIt};

    fn linkify_md() -> MarkdownIt {
        MarkdownIt::with_options(
            "default",
            serde_json::json!({ "linkify": true }).as_object().unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn returns_false_before_linkify_it_is_needed() {
        // node (tests/markdown_it/inline_rule_values.cjs, `linkify_early_returns`): with
        // linkify on, linkify(s) at the ':' (or 0), s.pending = the text before it: all false
        let md = linkify_md();
        for src in ["http", "a:/", ":/", "1://x", "://x"] {
            let mut env = Env::default();
            let mut st = StateInline::new(js::to_utf16(src), &md, &mut env, Vec::new());
            let colon = src.find(':').unwrap_or(0);
            st.pos = colon;
            st.pending = js::to_utf16(&src[..colon]);
            assert!(!linkify(&mut st, false), "{src:?}");
        }
        // and with linkify off it never looks
        let md = MarkdownIt::default();
        let mut env = Env::default();
        let mut st = StateInline::new(js::to_utf16("http://x"), &md, &mut env, Vec::new());
        st.pos = 4;
        st.pending = js::to_utf16("http");
        assert!(!linkify(&mut st, false));
    }

    #[test]
    fn a_scheme_before_colon_slash_slash_is_a_link() {
        // node: md.inline.State over `see http://x.org/a*` at the ':' with pending `see http`
        let md = linkify_md();
        let mut env = Env::default();
        let src = "see http://x.org/a*";
        let mut st = StateInline::new(js::to_utf16(src), &md, &mut env, Vec::new());
        st.pos = 8;
        st.pending = js::to_utf16("see http");
        assert!(linkify(&mut st, false));
        assert_eq!(st.pos, 18);
        assert_eq!(js::to_string(&st.pending), "");
        let kinds: Vec<_> = st.tokens.iter().map(|t| t.kind.as_str()).collect();
        assert_eq!(kinds, ["text", "link_open", "text", "link_close"]);
        assert_eq!(st.tokens[0].content, "see ");
        assert_eq!(st.tokens[2].content, "http://x.org/a");
    }
}
