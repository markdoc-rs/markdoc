//! The ast stage's way into `markdoc::next::index::parse_tokens`.

use markdoc::ast::Node;
use markdoc::markdown_it::Token;
use markdoc::next::index;
use markdoc::next::parser::ParserArgs;
use serde_json::{Map, Value};

/// `Markdoc.parse(tokens, args)` with the table transform. `src` is the text the tokens were
/// made from (the parser reads it for positions). `args` is upstream's ParserArgs as the
/// request carries it: `file`, `slots`, `location` (compare.py sends true, as the oracle
/// parses) and, when given, `conditionalTags`; the crate's own switches stay off, as
/// upstream has none. `Err("Name: message")` where upstream throws.
pub fn parse(tokens: &[Token], src: &str, args: &Map<String, Value>) -> Result<Node, String> {
    let args = ParserArgs {
        file: args.get("file").and_then(Value::as_str).map(str::to_owned),
        slots: args.get("slots").and_then(Value::as_bool).unwrap_or(false),
        location: args.get("location").and_then(Value::as_bool) != Some(false),
        conditional_tags: args
            .get("conditionalTags")
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .filter_map(|t| t.as_str().map(str::to_owned))
                    .collect()
            }),
        ..ParserArgs::default()
    };
    index::parse_tokens(tokens, src, &args).map_err(|e| index::thrown(&e).to_owned())
}
