//! Port of rules_core/strip_references.ts.

// Drop `reference_definition` tokens to keep the stream backward compatible
//
// Those tokens mark places link definitions took in the source. They are new,
// and plugins walking block tokens may not expect them, so by default the
// stream stays as it always was. Disable this rule to opt in.
//

use super::state_core::StateCore;

pub fn strip_references(state: &mut StateCore) {
    let tokens = &mut state.tokens;
    let mut last = 0;

    for curr in 0..tokens.len() {
        if tokens[curr].kind == "reference_definition" {
            continue;
        }

        // `tokens[last] = tokens[curr]`: what was at `last` (a dropped token, or one already
        // moved down) is never read again, so swapping it to `curr` is the same
        if curr != last {
            tokens.swap(last, curr);
        }

        last += 1;
    }

    // `tokens.length = last`
    if tokens.len() != last {
        tokens.truncate(last);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown_it::{Env, MarkdownIt, Token};

    fn kinds(tokens: &[Token]) -> Vec<&str> {
        tokens.iter().map(|t| t.kind.as_str()).collect()
    }

    fn run(kinds_in: &[&str]) -> Vec<Token> {
        let md = MarkdownIt::default();
        let mut env = Env::default();
        let mut state = StateCore::new(Vec::new(), &md, &mut env);
        state.tokens = kinds_in
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let mut t = Token::new(k, "", 0);
                t.content = i.to_string();
                t
            })
            .collect();
        strip_references(&mut state);
        state.tokens
    }

    #[test]
    fn drops_reference_definitions_and_keeps_the_order() {
        let r = "reference_definition";
        let out = run(&[r, "a", r, r, "b", "c", r]);
        assert_eq!(kinds(&out), ["a", "b", "c"]);
        // the tokens themselves move, not copies of the first one
        let contents: Vec<&str> = out.iter().map(|t| t.content.as_str()).collect();
        assert_eq!(contents, ["1", "4", "5"]);
        assert!(run(&[r, r]).is_empty());
        assert_eq!(kinds(&run(&["a", "b"])), ["a", "b"]);
        assert!(run(&[]).is_empty());
    }

    #[test]
    fn generated_token_sequences_match_node() {
        use crate::markdown_it::rules_core::smartquotes::gen_tokens::{
            Function, check, run, tok_syms,
        };
        let functions: [Function; 1] = [("strip_references", |o, m, c| {
            tok_syms(o, &run(&m[4], &[strip_references], c))
        })];
        check(&functions);
    }
}
