//! Port of parser_core.ts: the top-level rules executor. Glues block/inline parsers and
//! does intermediate transformations.

use super::ruler::Ruler;
use super::rules_core;
use super::rules_core::state_core::StateCore;

/// A core rule: `(state: StateCore) => void`.
pub type CoreRule = fn(&mut StateCore);

const RULES: [(&str, CoreRule); 8] = [
    ("normalize", rules_core::normalize::normalize),
    ("block", rules_core::block::block),
    (
        "strip_references",
        rules_core::strip_references::strip_references,
    ),
    ("inline", rules_core::inline::inline),
    ("linkify", rules_core::linkify::linkify),
    ("replacements", rules_core::replacements::replace),
    ("smartquotes", rules_core::smartquotes::smartquotes),
    // `text_join` finds `text_special` tokens (for escape sequences)
    // and joins them with the rest of the text
    ("text_join", rules_core::text_join::text_join),
];

/// `ParserCore` (`md.core`).
#[derive(Debug, Clone)]
pub struct ParserCore {
    /// Keeps the configuration of core rules.
    pub ruler: Ruler<CoreRule>,
}

impl Default for ParserCore {
    fn default() -> Self {
        Self::new()
    }
}

impl ParserCore {
    pub fn new() -> Self {
        let mut ruler = Ruler::new();

        for (name, rule) in RULES {
            ruler.push(name, rule, &[]);
        }
        ParserCore { ruler }
    }

    /// `process(state)`: executes core chain rules. Stops once a rule has thrown (see
    /// [`crate::markdown_it::JsError`]).
    pub fn process(&self, state: &mut StateCore) {
        let rules = self.ruler.get_rules("");

        for rule in rules {
            if state.env.has_thrown() {
                return;
            }
            rule(state);
        }
    }
}
