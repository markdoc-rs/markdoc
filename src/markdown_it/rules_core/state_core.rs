//! Port of rules_core/state_core.ts.

use crate::markdown_it::token::Token;
use crate::markdown_it::{Env, MarkdownIt};

/// Mutable state passed through the core rules chain.
///
/// `md` and `env` are borrowed for the whole parse (`env` is the caller's object, as in JS).
/// Core rules create tokens with `Token::new` (JS `new state.Token(...)`).
#[derive(Debug)]
pub struct StateCore<'a> {
    /// The source, as UTF-16 code units (JS string).
    pub src: Vec<u16>,
    pub env: &'a mut Env,
    pub tokens: Vec<Token>,
    pub inline_mode: bool,
    /// Link to parser instance.
    pub md: &'a MarkdownIt,
}

impl<'a> StateCore<'a> {
    pub fn new(src: Vec<u16>, md: &'a MarkdownIt, env: &'a mut Env) -> Self {
        StateCore {
            src,
            env,
            tokens: Vec::new(),
            inline_mode: false,
            md,
        }
    }
}
