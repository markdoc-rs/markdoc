//! Port of rules_block/paragraph.ts: paragraph.

use super::state_block::StateBlock;
use crate::markdown_it::common::utils::ascii_trim;
use crate::markdown_it::js;

/// `paragraph(state, startLine, endLine)`. JS ignores `silent` (the rule is in no alt
/// chain); so does this.
pub fn paragraph(
    state: &mut StateBlock,
    start_line: usize,
    end_line: usize,
    _silent: bool,
) -> bool {
    // Lines past the state's text (where JS reads `undefined` marks and goes on with NaN;
    // the parsers never call a rule there): no rule matches there.
    if start_line >= state.line_max {
        return false;
    }
    let end_line = end_line.min(state.line_max);
    let terminator_rules = state.md.block.ruler.get_rules("paragraph");
    let old_parent_type = state.parent_type;
    let mut next_line = start_line + 1;
    state.parent_type = "paragraph";

    // jump line-by-line until empty one or EOF
    // for (; nextLine < endLine && !state.isEmpty(nextLine); nextLine++)
    while next_line < end_line && !state.is_empty(next_line) {
        // this would be a code block normally, but after paragraph
        // it's considered a lazy continuation regardless of what's there
        if !state.md.options.allow_indentation && state.s_count[next_line] - state.blk_indent > 3 {
            next_line += 1;
            continue;
        }

        // quirk for blockquotes, this line should already be checked by that rule
        if state.s_count[next_line] < 0 {
            next_line += 1;
            continue;
        }

        // Some tags can terminate paragraph without empty line.
        let mut terminate = false;
        for &rule in terminator_rules {
            if rule(state, next_line, end_line, true) {
                terminate = true;
                break;
            }
        }
        if terminate {
            break;
        }
        next_line += 1;
    }

    let content = js::to_string(ascii_trim(&state.get_lines(
        start_line,
        next_line,
        state.blk_indent,
        false,
    )));

    state.line = next_line;

    // `token_*` borrow `state`: what JS reads from `state` after a push is read first.
    let line = state.line;

    let token_o = state.push("paragraph_open", "p", 1);
    token_o.map = Some([start_line, line]);

    let token_i = state.push("inline", "", 0);
    token_i.content = content;
    token_i.map = Some([start_line, line]);
    token_i.children = Some(Vec::new());

    state.push("paragraph_close", "p", -1);

    state.parent_type = old_parent_type;

    true
}

/// What the block rules' tests share: their cases are token streams the node oracle printed.
#[cfg(test)]
pub(crate) mod test_util {
    use crate::markdown_it::{AttrValue, Env, MarkdownIt, js};

    /// `(src, options as JSON, rules to disable as well, expected lines)`.
    pub(crate) type Case = (
        &'static str,
        &'static str,
        &'static [&'static str],
        &'static [&'static str],
    );

    fn quote(s: &str) -> String {
        serde_json::to_string(s).expect("a string serializes")
    }

    /// The block token stream of `src`, one line per token, then one per `env.references`
    /// label, in the form these node lines (the oracle bundle's engine) print:
    ///
    /// ```text
    /// md = new MarkdownIt('default', options); md.disable(['inline', 'strip_references', ...disable])
    /// for (t of md.parse(src, env = {})) `${t.type}/${t.tag}` + (t.map ? ` [${t.map}]` : '')
    ///     + ` L${t.level}` + (t.hidden ? ' hidden' : '') + markup, info, content as ` name=${JSON}`
    ///     + attrs as ` ${name}=${typeof v === 'number' ? v : JSON}` + (t.meta ? ` meta=${JSON}` : '')
    /// for (label of Object.keys(env.references).sort()) `ref ${label}=${JSON href} ${JSON title}`
    /// ```
    ///
    /// `inline` is off so the stream is the block rules' own; `strip_references` is off so
    /// the `reference_definition` tokens stay in it.
    pub(crate) fn block_tokens(src: &str, options: &str, disable: &[&str]) -> Vec<String> {
        let options: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(options).expect("options are a JSON object");
        let mut md = MarkdownIt::with_options("default", &options).expect("the default preset");
        let mut rules = vec!["inline", "strip_references"];
        rules.extend_from_slice(disable);
        md.disable(&rules, false).expect("known rules");
        let mut env = Env::default();
        let mut lines: Vec<String> = md
            .parse(src, &mut env)
            .iter()
            .map(|t| {
                let mut s = format!("{}/{}", t.kind, t.tag);
                if let Some([a, b]) = t.map {
                    s += &format!(" [{a},{b}]");
                }
                s += &format!(" L{}", t.level);
                if t.hidden {
                    s += " hidden";
                }
                for (name, value) in [
                    ("markup", &t.markup),
                    ("info", &t.info),
                    ("content", &t.content),
                ] {
                    if !value.is_empty() {
                        s += &format!(" {name}={}", quote(value));
                    }
                }
                for (name, value) in t.attrs.iter().flatten() {
                    let value = match value {
                        AttrValue::Str(v) => quote(v),
                        AttrValue::Num(n) => js::number_to_string(*n),
                    };
                    s += &format!(" {name}={value}");
                }
                if let Some(meta) = &t.meta {
                    s += &format!(" meta={meta}");
                }
                s
            })
            .collect();
        if let Some(references) = &env.references {
            let mut labels: Vec<&String> = references.keys().collect();
            labels.sort();
            for label in labels {
                let r = &references[label];
                lines.push(format!(
                    "ref {label}={} {}",
                    quote(&r.href),
                    quote(&r.title)
                ));
            }
        }
        lines
    }

    pub(crate) fn check(cases: &[Case]) {
        for &(src, options, disable, want) in cases {
            assert_eq!(
                block_tokens(src, options, disable),
                want,
                "{src:?} {options} {disable:?}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::markdown_it::rules_block::paragraph::test_util::{Case, check};

    #[test]
    fn paragraph_tokens_match_node() {
        // node: see `test_util` (block tokens, then env.references)
        let cases: &[Case] = &[
            (
                "  a \n b\n\nc",
                "{}",
                &[],
                &[
                    "paragraph_open/p [0,2] L0",
                    "inline/ [0,2] L1 content=\"a \\n b\"",
                    "paragraph_close/p L0",
                    "paragraph_open/p [3,4] L0",
                    "inline/ [3,4] L1 content=\"c\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "a\n    - b\n",
                "{}",
                &[],
                &[
                    "paragraph_open/p [0,2] L0",
                    "inline/ [0,2] L1 content=\"a\\n    - b\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "a\n    - b\n",
                "{\"allowIndentation\":true}",
                &[],
                &[
                    "paragraph_open/p [0,1] L0",
                    "inline/ [0,1] L1 content=\"a\"",
                    "paragraph_close/p L0",
                    "code_block/code [1,2] L0 content=\"- b\\n\"",
                ],
            ),
            (
                "\u{a0}a\u{a0}\n",
                "{}",
                &[],
                &[
                    "paragraph_open/p [0,1] L0",
                    "inline/ [0,1] L1 content=\"\u{a0}a\u{a0}\"",
                    "paragraph_close/p L0",
                ],
            ),
        ];
        check(cases);
    }
}
