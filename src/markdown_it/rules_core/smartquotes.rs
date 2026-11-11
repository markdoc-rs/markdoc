//! Port of rules_core/smartquotes.ts.

// Convert straight quotation marks to typographic ones
//

use std::collections::BTreeMap;

use super::state_core::StateCore;
use crate::markdown_it::common::utils::{is_md_ascii_punct, is_punct_char_code, is_white_space};
use crate::markdown_it::js;
use crate::markdown_it::token::Token;
use crate::markdown_it::{JsError, MarkdownIt, Quotes};

// QUOTE_TEST_RE = /['"]/ and QUOTE_RE = /['"]/g: matched by hand (`is_quote`)
const APOSTROPHE: u16 = 0x2019; /* ’ */

fn is_quote(c: u16) -> bool {
    c == 0x27 /* ' */ || c == 0x22 /* " */
}

struct Replacement {
    pos: usize,
    /// A JS value appended with `+`: `None` is `undefined` (a `quotes` too short), which
    /// becomes the text "undefined".
    ch: Option<Vec<u16>>,
}

/// `Record<string, Replacement[]>`, by token index (`Object.keys` lists integer keys in
/// ascending order; each token is rewritten on its own, so the order does not matter).
type ReplacementMap = BTreeMap<usize, Vec<Replacement>>;

struct StackItem {
    token: usize,
    pos: usize,
    single: bool,
    level: i32,
}

fn add_replacement(
    replacements: &mut ReplacementMap,
    token_idx: usize,
    pos: usize,
    ch: Option<Vec<u16>>,
) {
    // `if (!replacements[tokenIdx]) replacements[tokenIdx] = []`
    replacements
        .entry(token_idx)
        .or_default()
        .push(Replacement { pos, ch });
}

fn apply_replacements(str: &[u16], replacements: &mut [Replacement]) -> Vec<u16> {
    let mut result: Vec<u16> = Vec::new();
    let mut last_pos = 0;

    // `sort((a, b) => a.pos - b.pos)`, stable as V8's
    replacements.sort_by_key(|r| r.pos);

    for i in 0..replacements.len() {
        let replacement = &replacements[i];

        result.extend_from_slice(js::slice_to(str, last_pos, replacement.pos));
        match &replacement.ch {
            Some(ch) => result.extend_from_slice(ch),
            None => result.extend(js::to_utf16("undefined")),
        }
        last_pos = replacement.pos + 1;
    }

    result.extend_from_slice(js::slice_to(str, last_pos, str.len()));
    result
}

/// `tokens[j].content.charCodeAt(tokens[j].content.length - 1)` (the content is not empty),
/// the code unit JS reads where the engine made a lone surrogate there.
fn last_code_unit(token: &Token) -> u32 {
    u32::from(token.last_content_unit().expect("non-empty content"))
}

/// `tokens[j].content.charCodeAt(0)` (the content is not empty), as JS reads it.
fn first_code_unit(token: &Token) -> u32 {
    u32::from(token.first_content_unit().expect("non-empty content"))
}

/// `process_inlines(tokens, state)`; the tokens borrow the state, so this takes the
/// `state.md` it reads the `quotes` option from. `Err` where JS throws (reading `quotes[i]`
/// of a `null` `quotes` option), with nothing replaced.
fn process_inlines(tokens: &mut [Token], md: &MarkdownIt) -> Result<(), JsError> {
    let quotes: &Quotes = &md.options.quotes;
    let mut j: i64;

    let mut stack: Vec<StackItem> = Vec::new();
    // token index -> list of replacements in the original token content
    let mut replacements: ReplacementMap = BTreeMap::new();

    for i in 0..tokens.len() {
        let token = &tokens[i];

        let this_level = tokens[i].level;

        j = stack.len() as i64 - 1;
        while j >= 0 {
            if stack[j as usize].level <= this_level {
                break;
            }
            j -= 1;
        }
        // `stack.length = j + 1`
        stack.truncate((j + 1) as usize);

        if token.kind != "text" {
            continue;
        }

        let text = token.content_utf16();
        let mut pos = 0;
        let max = text.len();

        /* eslint no-labels:0,block-scoped-var:0 */
        'outer: while pos < max {
            // QUOTE_RE.lastIndex = pos; const t = QUOTE_RE.exec(text)
            let Some(t_index) = (pos..max).find(|&k| is_quote(text[k])) else {
                break;
            };
            let t0 = text[t_index];

            let mut can_open = true;
            let mut can_close = true;
            pos = t_index + 1;
            let is_single = t0 == 0x27; /* ' */

            // Find previous character,
            // default to space if it's the beginning of the line
            //
            let mut last_char: u32 = 0x20;

            if t_index as i64 - 1 >= 0 {
                last_char = u32::from(text[t_index - 1]);
            } else {
                j = i as i64 - 1;
                while j >= 0 {
                    let tj = &tokens[j as usize];
                    if tj.kind == "softbreak" || tj.kind == "hardbreak" {
                        break; // lastChar defaults to 0x20
                    }
                    if tj.content.is_empty() {
                        j -= 1;
                        continue; // should skip all tokens except 'text', 'html_inline' or 'code_inline'
                    }

                    last_char = last_code_unit(tj);
                    break;
                }
            }

            // Find next character,
            // default to space if it's the end of the line
            //
            let mut next_char: u32 = 0x20;

            if pos < max {
                next_char = u32::from(text[pos]);
            } else {
                j = i as i64 + 1;
                while (j as usize) < tokens.len() {
                    let tj = &tokens[j as usize];
                    if tj.kind == "softbreak" || tj.kind == "hardbreak" {
                        break; // nextChar defaults to 0x20
                    }
                    if tj.content.is_empty() {
                        j += 1;
                        continue; // should skip all tokens except 'text', 'html_inline' or 'code_inline'
                    }

                    next_char = first_code_unit(tj);
                    break;
                }
            }

            let is_last_punct_char = is_md_ascii_punct(last_char) || is_punct_char_code(last_char);
            let is_next_punct_char = is_md_ascii_punct(next_char) || is_punct_char_code(next_char);

            let is_last_white_space = is_white_space(last_char);
            let is_next_white_space = is_white_space(next_char);

            if is_next_white_space {
                can_open = false;
            } else if is_next_punct_char {
                if !(is_last_white_space || is_last_punct_char) {
                    can_open = false;
                }
            }

            if is_last_white_space {
                can_close = false;
            } else if is_last_punct_char {
                if !(is_next_white_space || is_next_punct_char) {
                    can_close = false;
                }
            }

            if next_char == 0x22 /* " */ && t0 == 0x22 {
                if last_char >= 0x30 /* 0 */ && last_char <= 0x39
                /* 9 */
                {
                    // special case: 1"" - count first quote as an inch
                    can_open = false;
                    can_close = false;
                }
            }

            if can_open && can_close {
                // Replace quotes in the middle of punctuation sequence, but not
                // in the middle of the words, i.e.:
                //
                // 1. foo " bar " baz - not replaced
                // 2. foo-"-bar-"-baz - replaced
                // 3. foo"bar"baz     - not replaced
                //
                can_open = is_last_punct_char;
                can_close = is_next_punct_char;
            }

            if !can_open && !can_close {
                // middle of word
                if is_single {
                    add_replacement(&mut replacements, i, t_index, Some(vec![APOSTROPHE]));
                }
                continue;
            }

            if can_close {
                // this could be a closing quote, rewind the stack to get a match
                j = stack.len() as i64 - 1;
                while j >= 0 {
                    let mut item = &stack[j as usize];
                    if stack[j as usize].level < this_level {
                        break;
                    }
                    if item.single == is_single && stack[j as usize].level == this_level {
                        item = &stack[j as usize];

                        let open_quote;
                        let close_quote;
                        if is_single {
                            open_quote = quotes.get(2)?;
                            close_quote = quotes.get(3)?;
                        } else {
                            open_quote = quotes.get(0)?;
                            close_quote = quotes.get(1)?;
                        }

                        add_replacement(&mut replacements, i, t_index, close_quote);
                        add_replacement(&mut replacements, item.token, item.pos, open_quote);

                        // `stack.length = j`
                        stack.truncate(j as usize);
                        continue 'outer;
                    }
                    j -= 1;
                }
            }

            if can_open {
                stack.push(StackItem {
                    token: i,
                    pos: t_index,
                    single: is_single,
                    level: this_level,
                });
            } else if can_close && is_single {
                add_replacement(&mut replacements, i, t_index, Some(vec![APOSTROPHE]));
            }
        }
    }

    for (idx, list) in replacements.iter_mut() {
        let content = tokens[*idx].content_utf16();
        tokens[*idx].set_content_utf16(&apply_replacements(&content, list));
    }
    Ok(())
}

pub fn smartquotes(state: &mut StateCore) {
    /* eslint max-depth:0 */
    if !state.md.options.typographer {
        return;
    }

    let md = state.md;
    for blk_idx in (0..state.tokens.len()).rev() {
        // QUOTE_TEST_RE.test(content): the quotes are ASCII, so the UTF-8 text can be searched
        if state.tokens[blk_idx].kind != "inline"
            || !state.tokens[blk_idx].content.contains(['\'', '"'])
        {
            continue;
        }

        // `children`: `tokens.length` of null (a plugin's inline token) throws in JS
        let Some(children) = state.tokens[blk_idx].children.as_mut() else {
            state.env.throw(JsError::type_error(
                "Cannot read properties of null (reading 'length')",
            ));
            return;
        };
        if let Err(error) = process_inlines(children, md) {
            state.env.throw(error);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown_it::Env;

    fn tok(kind: &str, content: &str, level: i32) -> Token {
        let mut t = Token::new(kind, "", 0);
        t.content = content.into();
        t.level = level;
        t
    }

    fn run(md: &MarkdownIt, children: Vec<Token>) -> Vec<String> {
        let mut env = Env::default();
        let mut state = StateCore::new(Vec::new(), md, &mut env);
        let mut inline = tok("inline", "'\"", 0);
        inline.children = Some(children);
        state.tokens = vec![inline];
        smartquotes(&mut state);
        state.tokens[0]
            .children
            .as_ref()
            .unwrap()
            .iter()
            .map(|t| t.content.clone())
            .collect()
    }

    fn typographer(quotes: Option<&str>) -> MarkdownIt {
        let mut opts = serde_json::Map::new();
        opts.insert("typographer".into(), true.into());
        if let Some(q) = quotes {
            opts.insert("quotes".into(), serde_json::from_str(q).unwrap());
        }
        MarkdownIt::with_options("default", &opts).unwrap()
    }

    #[test]
    fn quotes_pair_across_tokens_and_levels() {
        // node: smartquotes.ts on the same hand-built children
        // (tests/markdown_it/core_values.cjs prints them)
        let md = typographer(None);
        assert_eq!(
            run(
                &md,
                vec![
                    tok("text", "\"a", 0),
                    tok("code_inline", "x", 0),
                    tok("text", "b\" it's", 0)
                ]
            ),
            ["“a", "x", "b” it’s"]
        );
        // a closing quote at a deeper level does not match an opener outside it
        assert_eq!(
            run(
                &md,
                vec![
                    tok("text", "'a ", 0),
                    tok("em_open", "", 0),
                    tok("text", "b'", 1),
                    tok("em_close", "", 0),
                    tok("text", " c'", 0)
                ]
            ),
            ["‘a ", "", "b’", "", " c’"]
        );
        // softbreak resets the neighbouring character to a space
        assert_eq!(
            run(
                &md,
                vec![
                    tok("text", "x", 0),
                    tok("softbreak", "", 0),
                    tok("text", "\"y\"", 0)
                ]
            ),
            ["x", "", "“y”"]
        );
        // 1"" counts the first quote as an inch
        assert_eq!(run(&md, vec![tok("text", "1\"\" \"a\"", 0)]), ["1\"\" “a”"]);
        // astral and non-ASCII whitespace neighbours
        assert_eq!(
            run(
                &md,
                vec![tok("text", "\"\u{1F600}\" '\u{a0}x' \"\u{2018}\"", 0)]
            ),
            ["\u{201c}\u{1F600}\u{201d} \u{2019}\u{a0}x\u{2019} \u{201c}\u{2018}\u{201d}"]
        );
    }

    #[test]
    fn short_quotes_option_gives_undefined() {
        // `quotes[2]` past the end of the string is undefined, appended as text
        let md = typographer(Some("\"\u{ab}\u{bb}\""));
        assert_eq!(
            run(&md, vec![tok("text", "\"a\" 'b'", 0)]),
            ["«a» undefinedbundefined"]
        );
        let md = typographer(Some("[\"<<\", \">>\", \"<\", \">\"]"));
        assert_eq!(run(&md, vec![tok("text", "\"a\" 'b'", 0)]), ["<<a>> <b>"]);
    }

    #[test]
    fn astral_quotes_are_halves_of_pairs_and_become_u_fffd() {
        // Divergence (lone surrogates): `quotes[i]` of a string is a code unit, so with
        // astral quotes node puts half of a pair on each side ("\ud83da\ude00"); a Rust
        // string holds U+FFFD for each half.
        let md = typographer(Some("\"\u{1F600}\u{1F600}\""));
        assert_eq!(
            run(&md, vec![tok("text", "\"a\"", 0)]),
            ["\u{FFFD}a\u{FFFD}"]
        );
    }
}

/// The token sequences of tests/markdown_it/core_cases.json ("tokens") and node's digests of
/// the core rules' output on them.
///
/// `generate` mirrors `generateTokens` of tests/markdown_it/core_values.cjs step for step:
/// random children for one inline token (text with quotes, punctuation, astral and Unicode
/// whitespace; softbreaks, code and HTML, nested emphasis and (auto)links, images with alt
/// children, unbalanced closings), and block tokens around it. `check` runs the rules on them
/// as node does (under the same four `quotes` settings) and compares the digests; with
/// `MDIT_TOKENS_DUMP=FILE` it writes each case's symbols (`core_values.cjs --dump-tokens`).
#[cfg(test)]
pub(crate) mod gen_tokens {
    use std::io::Write;

    use crate::markdown_it::mdurl::gen_cases::{
        FNV_OFFSET, NULL, Syms, cases_json, fold, named_lists,
    };
    use crate::markdown_it::rules_core::state_core::StateCore;
    use crate::markdown_it::token::Token;
    use crate::markdown_it::{CoreRule, Env, MarkdownIt, js};

    struct Tok {
        kind: String,
        content: Vec<u16>,
        level: i32,
        info: String,
        children: Option<Vec<Tok>>,
    }

    pub struct Case {
        children: Vec<Tok>,
        before: Vec<String>,
        after: Vec<String>,
    }

    struct Rng(u32);

    impl Rng {
        fn rnd(&mut self, n: usize) -> usize {
            let mut s = self.0;
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            self.0 = s;
            (s % n as u32) as usize
        }
    }

    fn generate(v: &serde_json::Value) -> Vec<Case> {
        let pieces = named_lists(&v["pieces"]);
        let list = |name: &str| -> &Vec<Vec<u16>> {
            &pieces.iter().find(|(n, _)| n == name).expect("list").1
        };
        let pick = |rng: &mut Rng, name: &str| -> Vec<u16> {
            let l = list(name);
            l[rng.rnd(l.len())].clone()
        };
        let text = |rng: &mut Rng| -> Vec<u16> {
            let k = rng.rnd(3) + 1;
            let mut out = Vec::new();
            for _ in 0..k {
                out.extend(pick(rng, "text"));
            }
            out
        };
        let mut rng = Rng(v["seed"].as_u64().expect("seed") as u32);
        let mut cases = Vec::new();
        for _ in 0..v["cases"].as_u64().expect("cases") {
            let n = rng.rnd(9) + 1;
            let mut level = 0;
            let mut children = Vec::new();
            for _ in 0..n {
                let kind = js::to_string(&pick(&mut rng, "kind"));
                let mut t = Tok {
                    kind: kind.clone(),
                    content: Vec::new(),
                    level,
                    info: String::new(),
                    children: None,
                };
                if kind == "text" || kind == "text_special" {
                    t.content = text(&mut rng);
                } else if kind == "code_inline" {
                    t.content = pick(&mut rng, "code");
                } else if kind == "html_inline" {
                    t.content = pick(&mut rng, "html");
                } else if kind.ends_with("_open") {
                    t.level = level;
                    level += 1;
                    if kind == "link_open" && rng.rnd(2) == 0 {
                        t.info = "auto".into();
                    }
                } else if kind.ends_with("_close") {
                    level -= 1;
                    t.level = level;
                    if kind == "link_close" && rng.rnd(2) == 0 {
                        t.info = "auto".into();
                    }
                } else if kind == "image" {
                    let m = rng.rnd(4);
                    let mut alt = Vec::new();
                    for _ in 0..m {
                        let kind = if rng.rnd(3) == 0 {
                            "text_special"
                        } else {
                            "text"
                        };
                        let content = text(&mut rng);
                        alt.push(Tok {
                            kind: kind.into(),
                            content,
                            level: 0,
                            info: String::new(),
                            children: None,
                        });
                    }
                    t.children = Some(alt);
                }
                children.push(t);
            }
            let mut before = Vec::new();
            let mut after = Vec::new();
            for _ in 0..rng.rnd(3) {
                before.push(js::to_string(&pick(&mut rng, "block")));
            }
            for _ in 0..rng.rnd(3) {
                after.push(js::to_string(&pick(&mut rng, "block")));
            }
            cases.push(Case {
                children,
                before,
                after,
            });
        }
        cases
    }

    fn build(t: &Tok) -> Token {
        let mut x = Token::new(&t.kind, "", 0);
        x.content = js::to_string(&t.content);
        x.level = t.level;
        x.info = t.info.clone();
        x.children = t.children.as_ref().map(|c| c.iter().map(build).collect());
        x
    }

    /// The block tokens around one inline token, whose content is its children's text.
    fn block(c: &Case) -> Vec<Token> {
        let mut inline = Token::new("inline", "", 0);
        let children: Vec<Token> = c.children.iter().map(build).collect();
        inline.content = children.iter().map(|x| x.content.as_str()).collect();
        inline.children = Some(children);
        let plain = |kind: &String| {
            let mut x = Token::new(kind, "", 0);
            if kind == "inline" {
                x.children = Some(Vec::new());
            }
            x
        };
        let mut out: Vec<Token> = c.before.iter().map(plain).collect();
        out.push(inline);
        out.extend(c.after.iter().map(plain));
        out
    }

    /// The rules, in order, on a fresh core state holding the case's block tokens.
    pub fn run(md: &MarkdownIt, rules: &[CoreRule], c: &Case) -> Vec<Token> {
        let mut env = Env::default();
        let mut state = StateCore::new(Vec::new(), md, &mut env);
        state.tokens = block(c);
        for rule in rules {
            rule(&mut state);
        }
        state.tokens
    }

    pub fn tok_syms(o: &mut Syms, ts: &[Token]) {
        o.0.push(ts.len() as u32);
        for x in ts {
            o.str(&js::to_utf16(&x.kind));
            o.str(&js::to_utf16(&x.content));
            o.0.push((x.level + 1000) as u32);
            match &x.children {
                Some(c) => tok_syms(o, c),
                None => o.0.push(NULL),
            }
        }
    }

    /// The MarkdownIt instances core_values.cjs runs the rules under: `[0]` typographer with
    /// the default quotes, then `quotes` as '«»„“', as ['<<', '>>', '<', '>'] and as 'ab'
    /// (`quotes[2]` undefined); `[4]` the default preset as is.
    pub fn configs() -> Vec<MarkdownIt> {
        let with = |json: &str| {
            let opts: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(json).expect("options");
            MarkdownIt::with_options("default", &opts).expect("preset")
        };
        vec![
            with(r#"{"typographer": true}"#),
            with(r#"{"typographer": true, "quotes": "«»„“"}"#),
            with(r#"{"typographer": true, "quotes": ["<<", ">>", "<", ">"]}"#),
            with(r#"{"typographer": true, "quotes": "ab"}"#),
            MarkdownIt::default(),
        ]
    }

    pub type Function = (&'static str, fn(&mut Syms, &[MarkdownIt], &Case));

    /// Runs each function over every generated sequence and compares its digest with node's.
    pub fn check(functions: &[Function]) {
        let json = cases_json();
        let v = &json["tokens"];
        let cases = generate(v);
        assert_eq!(cases.len() as u64, v["count"].as_u64().expect("count"));
        let mds = configs();
        let mut inputs = FNV_OFFSET;
        let mut digests = vec![FNV_OFFSET; functions.len()];
        let mut dump = std::env::var_os("MDIT_TOKENS_DUMP")
            .map(|p| std::io::BufWriter::new(std::fs::File::create(p).expect("dump file")));
        for (i, c) in cases.iter().enumerate() {
            let mut s = Syms::default();
            tok_syms(&mut s, &block(c));
            inputs = s.0.iter().fold(inputs, |h, &x| fold(h, x));
            let mut outputs = serde_json::Map::new();
            for (k, (name, f)) in functions.iter().enumerate() {
                let mut o = Syms::default();
                f(&mut o, &mds, c);
                digests[k] = o.0.iter().fold(digests[k], |h, &x| fold(h, x));
                if dump.is_some() {
                    outputs.insert((*name).to_owned(), o.0.into());
                }
            }
            if let Some(d) = dump.as_mut() {
                let line = serde_json::json!({"i": i, "input": s.0, "outputs": outputs});
                writeln!(d, "{line}").expect("dump");
            }
        }
        let node = |name: &str| v["digests"][name].as_u64().expect("digest") as u32;
        assert_eq!(
            inputs,
            node("inputs"),
            "the generators disagree on the token sequences"
        );
        let differ: Vec<&str> = functions
            .iter()
            .zip(&digests)
            .filter(|((name, _), d)| **d != node(name))
            .map(|((name, _), _)| *name)
            .collect();
        assert!(
            differ.is_empty(),
            "over {} generated token sequences, the output digest differs from node's for {differ:?} \
             (MDIT_TOKENS_DUMP=FILE and core_values.cjs --dump-tokens show which)",
            cases.len()
        );
    }
}

#[cfg(test)]
mod generated_tests {
    use super::gen_tokens::{Function, check, run, tok_syms};
    use super::smartquotes;
    use crate::markdown_it::rules_core::{replacements, strip_references, text_join};

    #[test]
    fn generated_token_sequences_match_node() {
        let functions: [Function; 5] = [
            ("smartquotes_default", |o, m, c| {
                tok_syms(o, &run(&m[0], &[smartquotes], c))
            }),
            ("smartquotes_guillemets", |o, m, c| {
                tok_syms(o, &run(&m[1], &[smartquotes], c))
            }),
            ("smartquotes_array", |o, m, c| {
                tok_syms(o, &run(&m[2], &[smartquotes], c))
            }),
            ("smartquotes_short", |o, m, c| {
                tok_syms(o, &run(&m[3], &[smartquotes], c))
            }),
            ("chain", |o, m, c| {
                let rules = [
                    strip_references::strip_references,
                    replacements::replace,
                    smartquotes,
                    text_join::text_join,
                ];
                tok_syms(o, &run(&m[0], &rules, c))
            }),
        ];
        check(&functions);
    }
}
