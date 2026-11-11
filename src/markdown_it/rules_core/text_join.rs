//! Port of rules_core/text_join.ts.

// Join raw text tokens with the rest of the text
//
// This is set as a separate rule to provide an opportunity for plugins
// to run text replacements after text join, but before escape join.
//
// For example, `\:)` shouldn't be replaced with an emoji.
//

use super::state_core::StateCore;
use crate::markdown_it::JsError;
use crate::markdown_it::token::Token;

/// `tokens[curr + 1].content = tokens[curr].content + tokens[curr + 1].content`. The
/// token at `curr` is dropped right after, so its content is moved and the next one is
/// appended to it: a run of n text tokens then costs linear time, as JS's (rope) string
/// concatenation does, where prepending the run so far to each next token was quadratic.
/// Where either holds a lone surrogate the engine made, the two are joined as UTF-16, so
/// two halves that meet pair again as in JS.
fn collapse_into_next(tokens: &mut [Token], curr: usize) {
    let (prev, next) = tokens.split_at_mut(curr + 1);
    if Token::join_exact_into(&mut prev[curr], &mut next[0]) {
        return;
    }
    let mut joined = std::mem::take(&mut prev[curr].content);
    joined.push_str(&next[0].content);
    next[0].content = joined;
}

fn join_alt(tokens: &mut Vec<Token>) {
    let mut curr;
    let mut last;
    let max = tokens.len();

    curr = 0;
    while curr < max {
        if tokens[curr].kind == "text_special" {
            tokens[curr].kind = "text".into();
        }
        curr += 1;
    }

    curr = 0;
    last = 0;
    while curr < max {
        if tokens[curr].kind == "text" && curr + 1 < max && tokens[curr + 1].kind == "text" {
            collapse_into_next(tokens, curr);
        } else {
            // `tokens[last] = tokens[curr]` (strip_references.rs says why a swap is the same)
            if curr != last {
                tokens.swap(last, curr);
            }

            last += 1;
        }
        curr += 1;
    }

    // `tokens.length = last`
    if curr != last {
        tokens.truncate(last);
    }
}

pub fn text_join(state: &mut StateCore) {
    let mut curr;
    let mut last;
    let block_tokens = &mut state.tokens;
    let l = block_tokens.len();

    for j in 0..l {
        if block_tokens[j].kind != "inline" {
            continue;
        }

        // `blockTokens[j].children`: `tokens.length` of null (a plugin's inline token)
        // throws in JS
        let Some(tokens) = block_tokens[j].children.as_mut() else {
            state.env.throw(JsError::type_error(
                "Cannot read properties of null (reading 'length')",
            ));
            return;
        };
        let max = tokens.len();

        curr = 0;
        while curr < max {
            if tokens[curr].kind == "text_special" {
                tokens[curr].kind = "text".into();
            }

            // image `alt` is parsed into its own token tree
            if let Some(children) = tokens[curr].children.as_mut() {
                join_alt(children);
            }
            curr += 1;
        }

        curr = 0;
        last = 0;
        while curr < max {
            if tokens[curr].kind == "text" && curr + 1 < max && tokens[curr + 1].kind == "text" {
                // collapse two adjacent text nodes
                collapse_into_next(tokens, curr);
            } else {
                // `tokens[last] = tokens[curr]`
                if curr != last {
                    tokens.swap(last, curr);
                }

                last += 1;
            }
            curr += 1;
        }

        // `tokens.length = last`
        if curr != last {
            tokens.truncate(last);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown_it::{Env, MarkdownIt};

    fn tok(kind: &str, content: &str) -> Token {
        let mut t = Token::new(kind, "", 0);
        t.content = content.into();
        t
    }

    fn summary(tokens: &[Token]) -> Vec<String> {
        tokens
            .iter()
            .map(|t| {
                let mut s = format!("{}:{}", t.kind, t.content);
                if let Some(c) = &t.children {
                    s += &format!("{:?}", summary(c));
                }
                s
            })
            .collect()
    }

    fn run(block: Vec<Token>) -> Vec<Token> {
        let md = MarkdownIt::default();
        let mut env = Env::default();
        let mut state = StateCore::new(Vec::new(), &md, &mut env);
        state.tokens = block;
        text_join(&mut state);
        state.tokens
    }

    fn inline(children: Vec<Token>) -> Token {
        let mut t = tok("inline", "");
        t.children = Some(children);
        t
    }

    #[test]
    fn joins_text_and_text_special_runs() {
        // node: text_join.ts on the same tokens (tests/markdown_it/core_values.cjs --examples)
        let mut tail = tok("text", "d");
        tail.level = 3;
        let out = run(vec![
            tok("paragraph_open", ""),
            inline(vec![
                tok("text", "a"),
                tok("text_special", "\\*"),
                tok("text", "b"),
                tok("softbreak", ""),
                tok("text_special", "c"),
                tail,
                tok("code_inline", "x"),
                tok("text", "e"),
            ]),
            // text tokens outside an inline token are left alone
            tok("text", "p"),
            tok("text", "q"),
        ]);
        assert_eq!(
            summary(&out),
            [
                "paragraph_open:",
                r#"inline:["text:a\\*b", "softbreak:", "text:cd", "code_inline:x", "text:e"]"#,
                "text:p",
                "text:q",
            ]
        );
        // the merged token is the later one: it keeps its own fields
        assert_eq!(out[1].children.as_ref().unwrap()[2].level, 3);
    }

    #[test]
    fn joins_inside_image_alt_children() {
        let mut image = tok("image", "");
        image.children = Some(vec![
            tok("text_special", "&"),
            tok("text", "x"),
            tok("em_open", ""),
            tok("text", "y"),
            tok("text", "z"),
        ]);
        // an empty `children` array is truthy: join_alt runs on it, and does nothing; the
        // merged token is the later one, whose children are null
        let mut empty_children = tok("text", "t");
        empty_children.children = Some(Vec::new());
        let out = run(vec![inline(vec![image, empty_children, tok("text", "u")])]);
        assert_eq!(
            summary(&out),
            [r#"inline:["image:[\"text:&x\", \"em_open:\", \"text:yz\"]", "text:tu"]"#]
        );
    }

    #[test]
    fn inline_without_children_throws() {
        // node: text_join on [inline with children null] throws this TypeError
        let md = MarkdownIt::default();
        let mut env = Env::default();
        let mut state = StateCore::new(Vec::new(), &md, &mut env);
        state.tokens = vec![tok("inline", "x")];
        text_join(&mut state);
        assert_eq!(
            env.thrown,
            Some(JsError::type_error(
                "Cannot read properties of null (reading 'length')"
            ))
        );
    }

    #[test]
    fn generated_token_sequences_match_node() {
        use crate::markdown_it::rules_core::smartquotes::gen_tokens::{
            Function, check, run, tok_syms,
        };
        let functions: [Function; 1] = [("text_join", |o, m, c| {
            tok_syms(o, &run(&m[4], &[text_join], c))
        })];
        check(&functions);
    }
}
