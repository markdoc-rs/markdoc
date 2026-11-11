//! Port of parser_inline.ts: tokenizes paragraph content.

use super::ruler::Ruler;
use super::rules_inline;
use super::rules_inline::state_inline::StateInline;
use super::token::Token;
use super::{Env, JsError, MarkdownIt, js};

/// An inline rule: `(state, silent) => boolean`.
pub type InlineRule = fn(&mut StateInline, bool) -> bool;
/// A post-processing inline rule (`ruler2`): `(state) => void`.
pub type InlineRule2 = fn(&mut StateInline);

// Parser rules

const RULES: [(&str, InlineRule); 12] = [
    ("text", rules_inline::text::text),
    ("linkify", rules_inline::linkify::linkify),
    ("newline", rules_inline::newline::newline),
    ("escape", rules_inline::escape::escape),
    ("backticks", rules_inline::backticks::backtick),
    ("strikethrough", rules_inline::strikethrough::tokenize),
    ("emphasis", rules_inline::emphasis::tokenize),
    ("link", rules_inline::link::link),
    ("image", rules_inline::image::image),
    ("autolink", rules_inline::autolink::autolink),
    ("html_inline", rules_inline::html_inline::html_inline),
    ("entity", rules_inline::entity::entity),
];

// `rule2` ruleset was created specifically for emphasis/strikethrough
// post-processing and may be changed in the future.
//
// Don't use this for anything except pairs (plugins working with `balance_pairs`).
//
const RULES2: [(&str, InlineRule2); 4] = [
    ("balance_pairs", rules_inline::balance_pairs::link_pairs),
    ("strikethrough", rules_inline::strikethrough::post_process),
    ("emphasis", rules_inline::emphasis::post_process),
    // rules for pairs separate '**' into its own text tokens, which may be left unused,
    // rule below merges unused segments back with the rest of the text
    (
        "fragments_join",
        rules_inline::fragments_join::fragments_join,
    ),
];

/// `ParserInline` (`md.inline`).
#[derive(Debug, Clone)]
pub struct ParserInline {
    /// Keeps the configuration of inline rules.
    pub ruler: Ruler<InlineRule>,
    /// Second ruler used for post-processing (e.g. in emphasis-like rules).
    pub ruler2: Ruler<InlineRule2>,
}

impl Default for ParserInline {
    fn default() -> Self {
        Self::new()
    }
}

impl ParserInline {
    pub fn new() -> Self {
        let mut ruler = Ruler::new();
        let mut ruler2 = Ruler::new();

        for (name, rule) in RULES {
            ruler.push(name, rule, &[]);
        }

        for (name, rule) in RULES2 {
            ruler2.push(name, rule, &[]);
        }
        ParserInline { ruler, ruler2 }
    }

    /// `skipToken(state)`: skip a single token by running all rules in validation mode
    /// (moves `state.pos` past it, and caches where it ends).
    ///
    /// Throws (see [`crate::markdown_it::JsError`]) where JS throws: a rule that matched
    /// without moving `state.pos`, and nesting past [`crate::markdown_it::MAX_DEPTH`]. Once
    /// anything has thrown it moves `state.pos` to `state.posMax`, so that the scan calling it
    /// ends.
    pub fn skip_token(&self, state: &mut StateInline) {
        if !state.env.enter() {
            state.pos = state.pos.max(state.pos_max);
            return;
        }
        self.skip_token_inner(state);
        state.env.leave();
        if state.env.has_thrown() {
            state.pos = state.pos.max(state.pos_max);
        }
    }

    fn skip_token_inner(&self, state: &mut StateInline) {
        let pos = state.pos;
        let rules = self.ruler.get_rules("");
        let len = rules.len();
        let max_nesting = state.md.options.max_nesting;

        if let Some(&cached) = state.cache.get(&pos) {
            state.pos = cached;
            return;
        }

        let mut ok = false;

        if f64::from(state.level) < max_nesting {
            for i in 0..len {
                // Increment state.level and decrement it later to limit recursion.
                // It's harmless to do here, because no tokens are created. But ideally,
                // we'd need a separate private state variable for this purpose.
                //
                state.level += 1;
                ok = rules[i](state, true);
                state.level -= 1;

                if state.env.has_thrown() {
                    return;
                }
                if ok {
                    if pos >= state.pos {
                        state
                            .env
                            .throw(JsError::error("inline rule didn't increment state.pos"));
                        return;
                    }
                    break;
                }
            }
        } else {
            // Too much nesting, just skip until the end of the paragraph.
            //
            // NOTE: this will cause links to behave incorrectly in the following case,
            //       when an amount of `[` is exactly equal to `maxNesting + 1`:
            //
            //       [[[[[[[[[[[[[[[[[[[[[foo]()
            //
            // TODO: remove this workaround when CM standard will allow nested links
            //       (we can replace it by preventing links from being parsed in
            //       validation mode)
            //
            state.pos = state.pos_max;
        }

        if !ok {
            state.pos += 1;
        }
        state.cache.insert(pos, state.pos);
    }

    /// `tokenize(state)`: generate tokens for input range.
    ///
    /// Throws (see [`crate::markdown_it::JsError`]) where JS throws: a rule that matched
    /// without moving `state.pos`, and nesting past [`crate::markdown_it::MAX_DEPTH`]. Stops
    /// once anything has thrown.
    pub fn tokenize(&self, state: &mut StateInline) {
        if !state.env.enter() {
            return;
        }
        self.tokenize_inner(state);
        state.env.leave();
    }

    fn tokenize_inner(&self, state: &mut StateInline) {
        let rules = self.ruler.get_rules("");
        let len = rules.len();
        let end = state.pos_max;
        let max_nesting = state.md.options.max_nesting;

        while state.pos < end {
            // Try all possible rules.
            // On success, rule should:
            //
            // - update `state.pos`
            // - update `state.tokens`
            // - return true
            let prev_pos = state.pos;
            let mut ok = false;

            if f64::from(state.level) < max_nesting {
                for i in 0..len {
                    ok = rules[i](state, false);
                    if state.env.has_thrown() {
                        return;
                    }
                    if ok {
                        if prev_pos >= state.pos {
                            state
                                .env
                                .throw(JsError::error("inline rule didn't increment state.pos"));
                            return;
                        }
                        break;
                    }
                }
            }

            if ok {
                if state.pos >= end {
                    break;
                }
                continue;
            }

            // `state.pending += state.src[state.pos++]`: one UTF-16 code unit. Past the end
            // of `src` (only if a plugin set `posMax` beyond it) JS appends "undefined".
            match state.src.get(state.pos) {
                Some(&ch) => state.pending.push(ch),
                None => state.pending.extend(js::to_utf16("undefined")),
            }
            state.pos += 1;
        }

        if !state.pending.is_empty() {
            state.push_pending();
        }
    }

    /// `parse(str, md, env, outTokens)`: process input string and push inline tokens into
    /// `out_tokens`.
    pub fn parse(&self, src: &[u16], md: &MarkdownIt, env: &mut Env, out_tokens: &mut Vec<Token>) {
        let mut state = StateInline::new(src.to_vec(), md, env, std::mem::take(out_tokens));

        self.tokenize(&mut state);

        let rules = self.ruler2.get_rules("");
        let len = rules.len();

        for i in 0..len {
            if state.env.has_thrown() {
                break;
            }
            rules[i](&mut state);
        }
        *out_tokens = state.tokens;
    }
}
