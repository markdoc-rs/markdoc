//! Port of upstream Markdoc v0.5.11's `src/tokenizer/plugins/link.ts`.
//!
//! Owner: the plugins agent.
//!
//! A core rule (`link_url_validation`, after every other core rule): an `inline` token whose
//! content has a tag right after a URL of one of the validated protocols (no whitespace
//! between) gets an `href-format-invalid` error. Only the first closed tag of each inline
//! token is looked at (`containsMarkdocTagInUrl`).

use serde_json::{Map, Value};

use crate::markdown_it::{JsError, MarkdownIt, StateCore, Token, js};

use super::super::utils::{contains_markdoc_tag_in_url, text_before_first_closed_tag};
use super::super::{LinkPluginOptions, ValidatedProtocols};

pub const INVALID_HREF_MESSAGE: &str =
    "The 'href' format cannot contain Markdoc tag or variable. URLs must be static strings.";

/// Where [`plugin`] keeps its options on `md.options`: `{validatedProtocols: [...]}`, or `{}`
/// for protocols that are not iterable. Upstream's rule is a closure over them; a rule here
/// is a plain `fn`, so it reads them back from the parser it runs under.
pub const OPTIONS_KEY: &str = "$$markdocLinkPluginOptions";

/// `pushHrefError(token)`
fn push_href_error(token: &mut Token) {
    let mut error = Map::new();
    error.insert("id".into(), "href-format-invalid".into());
    error.insert("level".into(), "error".into());
    error.insert("message".into(), INVALID_HREF_MESSAGE.into());
    token
        .errors
        .get_or_insert_with(Vec::new)
        .push(Value::Object(error));
}

/// The validated protocols [`plugin`] was given.
fn validated_protocols(md: &MarkdownIt) -> ValidatedProtocols {
    match md
        .options
        .extra
        .get(OPTIONS_KEY)
        .and_then(|options| options.get("validatedProtocols"))
        .and_then(Value::as_array)
    {
        Some(protocols) => ValidatedProtocols::Iterable(
            protocols
                .iter()
                .filter_map(|p| p.as_str().map(str::to_owned))
                .collect(),
        ),
        None => ValidatedProtocols::NotIterable,
    }
}

/// `core(state, options)`
fn core(state: &mut StateCore) {
    if super::super::thrown(state.env) {
        return;
    }
    let protocols = validated_protocols(state.md);
    for token in &mut state.tokens {
        // `typeof token.content !== 'string'` is never true of a markdown-it token
        if token.kind != "inline" {
            continue;
        }

        let content = js::to_utf16(&token.content);
        let contains = match &protocols {
            ValidatedProtocols::Iterable(protocols) => {
                contains_markdoc_tag_in_url(&content, protocols)
            }
            ValidatedProtocols::NotIterable => {
                // `for (const protocol of allowedProtocols)` is reached at the first closed tag
                if text_before_first_closed_tag(&content).is_some() {
                    state
                        .env
                        .throw(JsError::type_error("allowedProtocols is not iterable"));
                    return;
                }
                false
            }
        };
        if contains {
            push_href_error(token);
        }
    }
}

/// `plugin(md, options)`
pub fn plugin(md: &mut MarkdownIt, options: &LinkPluginOptions) {
    let mut stored = Map::new();
    if let ValidatedProtocols::Iterable(protocols) = &options.validated_protocols {
        stored.insert(
            "validatedProtocols".into(),
            Value::Array(protocols.iter().map(|p| Value::String(p.clone())).collect()),
        );
    }
    md.options
        .extra
        .insert(OPTIONS_KEY.into(), Value::Object(stored));
    md.core.ruler.push("link_url_validation", core, &[]);
}
