//! Port of parser_block.ts: the block-level tokenizer.

use super::ruler::Ruler;
use super::rules_block;
use super::rules_block::state_block::StateBlock;
use super::token::Token;
use super::{Env, JsError, MarkdownIt};

/// A block rule: `(state, startLine, endLine, silent) => boolean`.
pub type BlockRule = fn(&mut StateBlock, usize, usize, bool) -> bool;

// First 2 params - rule name & source. Secondary array - list of rules,
// which can be terminated by this one.
const RULES: [(&str, BlockRule, &[&str]); 11] = [
    (
        "table",
        rules_block::table::table,
        &["paragraph", "reference"],
    ),
    ("code", rules_block::code::code, &[]),
    (
        "fence",
        rules_block::fence::fence,
        &["paragraph", "reference", "blockquote", "list"],
    ),
    (
        "blockquote",
        rules_block::blockquote::blockquote,
        &["paragraph", "reference", "blockquote", "list"],
    ),
    (
        "hr",
        rules_block::hr::hr,
        &["paragraph", "reference", "blockquote", "list"],
    ),
    (
        "list",
        rules_block::list::list,
        &["paragraph", "reference", "blockquote"],
    ),
    ("reference", rules_block::reference::reference, &[]),
    (
        "html_block",
        rules_block::html_block::html_block,
        &["paragraph", "reference", "blockquote"],
    ),
    (
        "heading",
        rules_block::heading::heading,
        &["paragraph", "reference", "blockquote"],
    ),
    ("lheading", rules_block::lheading::lheading, &[]),
    ("paragraph", rules_block::paragraph::paragraph, &[]),
];

/// `ParserBlock` (`md.block`).
#[derive(Debug, Clone)]
pub struct ParserBlock {
    /// Keeps the configuration of block rules.
    pub ruler: Ruler<BlockRule>,
}

impl Default for ParserBlock {
    fn default() -> Self {
        Self::new()
    }
}

impl ParserBlock {
    pub fn new() -> Self {
        let mut ruler = Ruler::new();

        for (name, rule, alt) in RULES {
            ruler.push(name, rule, alt);
        }
        ParserBlock { ruler }
    }

    /// `tokenize(state, startLine, endLine)`: generate tokens for input range.
    ///
    /// Throws (see [`crate::markdown_it::JsError`]) where JS throws: a rule that matched
    /// without moving `state.line`, no rule matching at all (only possible when the paragraph
    /// rule is disabled), and nesting past [`crate::markdown_it::MAX_DEPTH`]. Stops once
    /// anything has thrown.
    ///
    /// Lines past `state.line_max` (where JS reads `undefined` marks and goes on with NaN)
    /// end the range.
    pub fn tokenize(&self, state: &mut StateBlock, start_line: usize, end_line: usize) {
        if !state.env.enter() {
            return;
        }
        let end_line = end_line.min(state.line_max);
        self.tokenize_lines(state, start_line, end_line);
        state.env.leave();
    }

    fn tokenize_lines(&self, state: &mut StateBlock, start_line: usize, end_line: usize) {
        let rules = self.ruler.get_rules("");
        let len = rules.len();
        let max_nesting = state.md.options.max_nesting;
        let mut line = start_line;
        let mut has_empty_lines = false;

        while line < end_line {
            line = state.skip_empty_lines(line);
            state.line = line;
            if line >= end_line {
                break;
            }

            // Termination condition for nested calls.
            // Nested calls currently used for blockquotes & lists
            if state.s_count[line] < state.blk_indent {
                break;
            }

            // If nesting level exceeded - skip tail to the end. That's not ordinary
            // situation and we should not care about content.
            if f64::from(state.level) >= max_nesting {
                state.line = end_line;
                break;
            }

            // Try all possible rules.
            // On success, rule should:
            //
            // - update `state.line`
            // - update `state.tokens`
            // - return true
            let prev_line = state.line;
            let mut ok = false;

            for i in 0..len {
                ok = rules[i](state, line, end_line, false);
                if state.env.has_thrown() {
                    return;
                }
                if ok {
                    if prev_line >= state.line {
                        state
                            .env
                            .throw(JsError::error("block rule didn't increment state.line"));
                        return;
                    }
                    break;
                }
            }

            // this can only happen if user disables paragraph rule
            if !ok {
                state
                    .env
                    .throw(JsError::error("none of the block rules matched"));
                return;
            }

            // set state.tight if we had an empty line before current tag
            // i.e. latest empty line should not count
            state.tight = !has_empty_lines;

            // paragraph might "eat" one newline after it in nested lists
            if state.is_empty(state.line - 1) {
                has_empty_lines = true;
            }

            line = state.line;

            if line < end_line && state.is_empty(line) {
                has_empty_lines = true;
                line += 1;
                state.line = line;
            }
        }
    }

    /// `parse(src, md, env, outTokens)`: process input string and push block tokens into
    /// `out_tokens`.
    pub fn parse(&self, src: &[u16], md: &MarkdownIt, env: &mut Env, out_tokens: &mut Vec<Token>) {
        if src.is_empty() {
            return;
        }

        let mut state = StateBlock::new(src.to_vec(), md, env, std::mem::take(out_tokens));

        let (line, line_max) = (state.line, state.line_max);
        self.tokenize(&mut state, line, line_max);
        *out_tokens = state.tokens;
    }
}
