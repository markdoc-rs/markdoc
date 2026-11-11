//! Port of rules_core/block.ts.

use super::state_core::StateCore;
use crate::markdown_it::js;
use crate::markdown_it::token::Token;

pub fn block(state: &mut StateCore) {
    if state.inline_mode {
        let mut token = Token::new("inline", "", 0);
        token.content = js::to_string(&state.src);
        token.map = Some([0, 1]);
        token.children = Some(Vec::new());
        state.tokens.push(token);
    } else {
        let md = state.md;
        md.block.parse(&state.src, md, state.env, &mut state.tokens);
    }
}
