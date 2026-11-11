//! Port of rules_core/inline.ts.

use super::state_core::StateCore;
use crate::markdown_it::{JsError, js};

pub fn inline(state: &mut StateCore) {
    let md = state.md;
    let tokens = &mut state.tokens;

    // Parse inlines
    for i in 0..tokens.len() {
        let tok = &mut tokens[i];
        if tok.kind == "inline" {
            // `tok.children!`: JS hands the children array to the inline state, which fills
            // it in place. A null `children` (a plugin's inline token) throws in StateInline's
            // constructor (`outTokens.length`).
            let Some(mut children) = tok.children.take() else {
                state.env.throw(JsError::type_error(
                    "Cannot read properties of null (reading 'length')",
                ));
                return;
            };
            md.inline
                .parse(&js::to_utf16(&tok.content), md, state.env, &mut children);
            tok.children = Some(children);
        }
    }
}
