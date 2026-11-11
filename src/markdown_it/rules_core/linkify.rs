//! Port of rules_core/linkify.ts.
//!
//! Where JS throws a TypeError (a `null` children list, `match` answering `null` where
//! `test` said yes, a `link_close` with nothing before it at its level), the port records it
//! on the environment (see [`crate::markdown_it::JsError`]) and returns.

// Replace link-like texts with link nodes.
//
// Currently restricted by `md.validateLink()` to http/https/ftp
//

use super::state_core::StateCore;
use crate::markdown_it::js;
use crate::markdown_it::linkify_it::starts_with_mailto;
use crate::markdown_it::token::{AttrValue, Token};
use crate::markdown_it::{JsError, MarkdownIt};

/// `state.md.linkify.test(text)`; `Err` where it throws.
fn linkify_test(md: &MarkdownIt, text: &[u16]) -> Result<bool, JsError> {
    md.linkify.test(text)
}

fn is_ascii_ci(c: u16, lower: u8) -> bool {
    c == u16::from(lower) || c == u16::from(lower.to_ascii_uppercase())
}

/// `/^<a[>\s]/i.test(str)`.
fn is_link_open(str: &[u16]) -> bool {
    str.len() >= 3
        && str[0] == u16::from(b'<')
        && is_ascii_ci(str[1], b'a')
        && (str[2] == u16::from(b'>') || js::is_js_space(u32::from(str[2])))
}

/// `/^<\/a\s*>/i.test(str)`.
fn is_link_close(str: &[u16]) -> bool {
    if !(str.len() >= 3
        && str[0] == u16::from(b'<')
        && str[1] == u16::from(b'/')
        && is_ascii_ci(str[2], b'a'))
    {
        return false;
    }
    let spaces = str[3..]
        .iter()
        .take_while(|&&c| js::is_js_space(u32::from(c)))
        .count();
    str.get(3 + spaces) == Some(&u16::from(b'>'))
}

/// `tokens[i]`, where JS reads `undefined` below index 0 (and then throws).
fn at(tokens: &[Token], i: i64) -> Option<&Token> {
    usize::try_from(i).ok().and_then(|i| tokens.get(i))
}

fn type_error(property: &str, of: &str) -> JsError {
    JsError::type_error(format!(
        "Cannot read properties of {of} (reading '{property}')"
    ))
}

pub fn linkify(state: &mut StateCore) {
    let md = state.md;
    let block_tokens = &mut state.tokens;

    if !md.options.linkify {
        return;
    }

    let l = block_tokens.len();
    for j in 0..l {
        if block_tokens[j].kind != "inline" {
            continue;
        }
        match linkify_test(md, &js::to_utf16(&block_tokens[j].content)) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                state.env.throw(error);
                return;
            }
        }

        let Some(tokens) = block_tokens[j].children.as_ref() else {
            state.env.throw(type_error("length", "null"));
            return;
        };
        // { index, nodes }
        let mut replacements: Vec<(usize, Vec<Token>)> = Vec::new();

        let mut html_link_level: i32 = 0;

        // We scan from the end, to keep position when new tags added.
        // Use reversed logic in links start/end match
        let mut i: i64 = tokens.len() as i64 - 1;
        while i >= 0 {
            // `for (...; i >= 0; i--)`: a `continue` there is a `break 'body` here
            'body: {
                let current_token = &tokens[i as usize];

                // Skip content of markdown links
                if current_token.kind == "link_close" {
                    i -= 1;
                    loop {
                        let Some(token) = at(tokens, i) else {
                            state.env.throw(type_error("level", "undefined"));
                            return;
                        };
                        if token.level == current_token.level || token.kind == "link_open" {
                            break;
                        }
                        i -= 1;
                    }
                    break 'body;
                }

                // Skip content of html tag links
                if current_token.kind == "html_inline" {
                    let content = js::to_utf16(&current_token.content);
                    if is_link_open(&content) && html_link_level > 0 {
                        html_link_level -= 1;
                    }
                    if is_link_close(&content) {
                        html_link_level += 1;
                    }
                }
                if html_link_level > 0 {
                    break 'body;
                }

                let text = if current_token.kind == "text" {
                    current_token.content_utf16()
                } else {
                    Vec::new()
                };
                let found = current_token.kind == "text"
                    && match linkify_test(md, &text) {
                        Ok(found) => found,
                        Err(error) => {
                            state.env.throw(error);
                            return;
                        }
                    };
                if found {
                    // `state.md.linkify.match(text)!`: `links.length` below throws on null
                    let mut links = match md.linkify.match_all(&text) {
                        Ok(Some(links)) => links,
                        Ok(None) => {
                            state.env.throw(type_error("length", "null"));
                            return;
                        }
                        Err(error) => {
                            state.env.throw(error);
                            return;
                        }
                    };

                    // Now split string to nodes
                    let mut nodes: Vec<Token> = Vec::new();
                    let mut level = current_token.level;
                    let mut last_pos = 0;

                    // forbid escape sequence at the start of the string,
                    // this avoids http\://example.com/ from being linkified as
                    // http:<a href="//example.com/">//example.com/</a>
                    if !links.is_empty()
                        && links[0].index == 0
                        && i > 0
                        && tokens[(i - 1) as usize].kind == "text_special"
                    {
                        links = links[1..].to_vec();
                    }

                    for ln in 0..links.len() {
                        let url: &Vec<u16> = &links[ln].url;
                        let full_url = md.normalize_link(url);
                        if !md.validate_link(&full_url) {
                            continue;
                        }

                        let mut url_text = links[ln].text.clone();

                        // Linkifier might send raw hostnames like "example.com", where url
                        // starts with domain name. So we prepend http:// in those cases,
                        // and remove it afterwards.
                        //
                        if links[ln].schema.is_empty() {
                            let mut prefixed = js::to_utf16("http://");
                            prefixed.extend_from_slice(&url_text);
                            // .replace(/^http:\/\//, '')
                            url_text = strip_prefix(md.normalize_link_text(&prefixed), "http://");
                        } else if links[ln].schema == js::to_utf16("mailto:")
                            && !starts_with_mailto(&url_text)
                        {
                            let mut prefixed = js::to_utf16("mailto:");
                            prefixed.extend_from_slice(&url_text);
                            // .replace(/^mailto:/, '')
                            url_text = strip_prefix(md.normalize_link_text(&prefixed), "mailto:");
                        } else {
                            url_text = md.normalize_link_text(&url_text);
                        }

                        let pos = links[ln].index;

                        if pos > last_pos {
                            let mut token = Token::new("text", "", 0);
                            token.set_content_utf16(js::slice_to(&text, last_pos, pos));
                            token.level = level;
                            nodes.push(token);
                        }

                        let mut token_o = Token::new("link_open", "a", 1);
                        token_o.attrs = Some(vec![(
                            "href".to_owned(),
                            AttrValue::Str(js::to_string(&full_url)),
                        )]);
                        token_o.level = level;
                        level += 1;
                        token_o.markup = "linkify".into();
                        token_o.info = "auto".into();
                        nodes.push(token_o);

                        let mut token_t = Token::new("text", "", 0);
                        token_t.set_content_utf16(&url_text);
                        token_t.level = level;
                        nodes.push(token_t);

                        let mut token_c = Token::new("link_close", "a", -1);
                        level -= 1;
                        token_c.level = level;
                        token_c.markup = "linkify".into();
                        token_c.info = "auto".into();
                        nodes.push(token_c);

                        last_pos = links[ln].last_index;
                    }
                    if last_pos < text.len() {
                        let mut token = Token::new("text", "", 0);
                        token.set_content_utf16(js::slice_to(&text, last_pos, text.len()));
                        token.level = level;
                        nodes.push(token);
                    }

                    replacements.push((i as usize, nodes));
                }
            }
            i -= 1;
        }

        if !replacements.is_empty() {
            let mut new_tokens_length = tokens.len() as i64;
            for replacement in &replacements {
                new_tokens_length += replacement.1.len() as i64 - 1;
            }

            // `new Array(newTokensLength)`, every slot of which is filled below
            let mut new_tokens: Vec<Token> = Vec::with_capacity(new_tokens_length.max(0) as usize);

            replacements.reverse();

            // `replacements[replacementIndex]`, moving on once it is used
            let mut replacements = replacements.into_iter().peekable();
            let old_tokens = block_tokens[j].children.take().unwrap_or_default();
            for (i, token) in old_tokens.into_iter().enumerate() {
                // `replacement?.index === i`
                if replacements
                    .peek()
                    .is_some_and(|replacement| replacement.0 == i)
                {
                    let (_, nodes) = replacements.next().expect("peeked");
                    for node in nodes {
                        new_tokens.push(node);
                    }
                } else {
                    new_tokens.push(token);
                }
            }

            block_tokens[j].children = Some(new_tokens);
        }
    }
}

/// `s.replace(/^<prefix>/, '')` for a literal, case-sensitive prefix.
fn strip_prefix(s: Vec<u16>, prefix: &str) -> Vec<u16> {
    let p = js::to_utf16(prefix);
    if s.starts_with(&p) {
        s[p.len()..].to_vec()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown_it::Env;

    #[test]
    fn html_link_tests_match_the_regexes() {
        let t = |s: &str| js::to_utf16(s);
        assert!(
            is_link_open(&t("<a href>"))
                && is_link_open(&t("<A>"))
                && is_link_open(&t("<a\u{3000}x"))
        );
        assert!(!is_link_open(&t("<ab>")) && !is_link_open(&t("<a")));
        assert!(
            is_link_close(&t("</a>"))
                && is_link_close(&t("</A \t\n>"))
                && is_link_close(&t("</a\u{feff}>x"))
        );
        assert!(!is_link_close(&t("</ab>")) && !is_link_close(&t("</a")));
        assert!(starts_with_mailto(&t("MailTo:x")) && !starts_with_mailto(&t("mailto")));
    }

    #[test]
    fn inert_without_the_linkify_option() {
        let md = MarkdownIt::default();
        let mut env = Env::default();
        let mut state = StateCore::new(Vec::new(), &md, &mut env);
        let mut inline = Token::new("inline", "", 0);
        inline.content = "see https://example.com".into();
        inline.children = Some(Vec::new());
        state.tokens = vec![inline];
        linkify(&mut state);
        assert_eq!(state.tokens[0].children.as_deref(), Some(&[][..]));
    }

    #[test]
    fn a_link_close_alone_at_its_level_throws_as_js_reads_undefined() {
        let mut opts = serde_json::Map::new();
        opts.insert("linkify".into(), true.into());
        let md = MarkdownIt::with_options("default", &opts).unwrap();
        let mut env = Env::default();
        let mut state = StateCore::new(Vec::new(), &md, &mut env);
        let mut inline = Token::new("inline", "", 0);
        inline.content = "http://x.org".into();
        let mut close = Token::new("link_close", "a", -1);
        close.level = 3;
        inline.children = Some(vec![Token::new("text", "", 0), close]);
        state.tokens = vec![inline];
        linkify(&mut state);
        assert_eq!(
            env.thrown,
            Some(JsError::type_error(
                "Cannot read properties of undefined (reading 'level')"
            ))
        );
    }
}
