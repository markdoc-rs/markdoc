//! A JavaScript `RegExp`, as a schema attribute's `matches` holds one.
//!
//! Owner: the semantics agent (the regex engine: package D of the closing round).
//!
//! Upstream's validator calls `matches.test(value)` and prints `${matches}`. Both are
//! JavaScript's, as node 22's V8 (12.4, ICU 78.2, Unicode 17.0) runs them, and so are they
//! here, on every input:
//!
//! - **The pattern is read by V8's own parser, ported** (`v8.rs`, from V8's
//!   `regexp-parser.cc`): a pattern V8 rejects is a `SyntaxError` with V8's message, found
//!   where V8 finds it (`Invalid regular expression: /(/: Unterminated group`), and a pattern
//!   V8 accepts means what V8 reads in it, Annex B and all (octal escapes, `\c`, `\k`
//!   without groups, `{` as a character, quantified lookaheads, ...). The property names V8
//!   accepts are `names.rs`, generated from node.
//! - **The match is regress's**, vendored (`vendor/regress`, see its `VENDORED.md`), given
//!   the parse written out in a canonical spelling it reads the same way. The vendored copy
//!   carries fixes of regress's departures from ECMAScript (case folding without `u`; `v`
//!   classes folding each operand, their one-character and empty strings, negated classes;
//!   `\W` with `iu`; backreferences ending inside a pair) and, opt-in through
//!   `Flags::v8_compat`, V8's departures from ECMAScript: its search steps one code unit at
//!   a time and so finds an empty match between the halves of a pair (`/\B/gu` on `a😀xab`
//!   at 2); `\p{ASCII}` and one-character `\q{}` strings are not case folded with `iv`;
//!   a case-insensitive backreference with `u` or `v` in a subject with a code unit past
//!   U+00FF compares by ICU's full case folding (`/(ﬀf)\1/iu` matches `ﬀffﬀ`).
//! - **`lastIndex`** is kept for `g` and `y` as JS keeps it: a regex object is shared by
//!   every node it validates (in upstream as here, clones of a config share it), V8 steps back
//!   from a `lastIndex` inside a surrogate pair except for a pattern its atom path searches
//!   (see [`JsRegExp::test`]), and a sticky regex tries `lastIndex` alone.
//! - **A match that needs more backtracking than V8's stack holds** throws V8's
//!   `RangeError: Maximum call stack size exceeded` ([`BACKTRACK_LIMIT`]).
//!
//! Checked against node, for every code point: case-insensitive matching in each mode,
//! every property name node accepts with and without `i`, `\w`, `\d`, `\s` and `.`
//! (`findings/conformance/work/regex/tables/gen_scans.cjs`); and on random patterns, inputs
//! and `lastIndex` sequences over every flag combination
//! (`findings/conformance/work/regex/fuzz/`). `CONFORMANCE.md` has the counts.
//!
//! # Divergences from upstream
//!
//! - **Stack bands**, as for the rest of the port (`CONFORMANCE.md`, "Call stack depth"):
//!   V8 fails where its stacks run out and the port has fixed limits. A match whose
//!   backtracking needs more than [`BACKTRACK_LIMIT`] steps throws `RangeError` in both, at
//!   the same count for the simplest loops (`(?:a|){N}`, `(?:x?){N}` from 3,355,443) and
//!   apart for others (`(a|){N}`: node from 2,396,745, the port from 1,118,481); where a
//!   pattern that big is reached at all also depends on each engine's shortcuts (V8 fails
//!   early when the subject is shorter than the pattern needs; regress does not). Groups that
//!   capture or look around nest at most 255 deep in regress, `v` classes 256 (`not
//!   modeled` past them); node takes about 10,400 capturing groups (deeper: `SyntaxError:
//!   Stack overflow` from `test`), 6,943 classes (deeper: `SyntaxError: Maximum call stack
//!   size exceeded`, which the port gives past 1,000), and non-capturing groups without
//!   limit (the port too: it writes them out flat).
//! - **A case-insensitive backreference with `u` or `v` in a subject with code units up to
//!   U+00FF only**: V8 compares by Latin-1 case pairs when it stores the subject one byte a
//!   unit, and by ICU's full case folding when it stores it two bytes a unit, which depends
//!   on how the string was made (a long slice of a two-byte string stays two-byte), not on
//!   its value; the port always takes the first. They differ only for `ß` against `s`
//!   (`/(ßs)\1/iu` on `ßssß`).

use std::sync::atomic::{AtomicUsize, Ordering};

mod names;
mod v8;

/// `new RegExp(pattern, flags)`.
#[derive(Debug)]
pub struct JsRegExp {
    /// `[[OriginalSource]]`: the pattern as given.
    pattern: String,
    /// The `flags` getter's string: the flags in the order `dgimsuvy`.
    flags: String,
    global: bool,
    sticky: bool,
    /// Does a `lastIndex` inside a surrogate pair start the search at the pair (see
    /// [`JsRegExp::test`])?
    steps_back: bool,
    regex: regress::Regex,
    last_index: AtomicUsize,
}

/// What `regexp.exec` found: UTF-16 code-unit ranges, as `match.index`, `match.indices` give.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsMatch {
    /// The whole match.
    pub range: std::ops::Range<usize>,
    /// Each capture group's range, in group order; `None` for a group that did not take part.
    pub captures: Vec<Option<std::ops::Range<usize>>>,
}

/// How many entries regress's backtrack stack may hold before a match throws, as V8's does
/// once its backtrack stack (64 MiB) is full: node 22 throws `RangeError: Maximum call stack
/// size exceeded` from 3,355,443 iterations of `(?:a|){N}` or `(?:x?){N}` (V8 pushes 20
/// bytes a step), and so does the port (regress keeps one entry a step there, and three at
/// the start). Other shapes take more or less on either side: `\1{N}` with `\1` empty from
/// 3,355,443 in node and 3,355,445 here, `(a|){N}` from 2,396,745 in node and 1,118,481
/// here. At most about 130 MiB of stack.
pub const BACKTRACK_LIMIT: usize = 3_355_445;

/// The flags `RegExp` knows, in the order its `flags` getter writes them.
const FLAG_ORDER: &str = "dgimsuvy";

impl JsRegExp {
    /// `new RegExp(pattern, flags)`; `Err` where JS throws a `SyntaxError` (an unknown or
    /// repeated flag, `u` with `v`, a pattern V8 rejects), with V8's message.
    pub fn new(pattern: &str, flags: &str) -> Result<JsRegExp, String> {
        let mut seen = String::new();
        for c in flags.chars() {
            if !FLAG_ORDER.contains(c) || seen.contains(c) {
                return Err(format!(
                    "SyntaxError: Invalid flags supplied to RegExp constructor '{flags}'"
                ));
            }
            seen.push(c);
        }
        let has = |c: char| seen.contains(c);
        if has('u') && has('v') {
            return Err(format!(
                "SyntaxError: Invalid flags supplied to RegExp constructor '{flags}'"
            ));
        }
        let full_unicode = has('u') || has('v');
        let ordered: String = FLAG_ORDER.chars().filter(|&c| has(c)).collect();
        let v8_flags = v8::Flags {
            unicode: has('u'),
            sets: has('v'),
        };
        let units: Vec<u16> = pattern.encode_utf16().collect();
        let parsed = v8::parse(&units, v8_flags).map_err(|e| {
            format!(
                "SyntaxError: Invalid regular expression: /{pattern}/{ordered}: {}",
                e.message()
            )
        })?;
        let compile_flags = regress::Flags {
            icase: has('i'),
            multiline: has('m'),
            dot_all: has('s'),
            no_opt: false,
            unicode: full_unicode,
            unicode_sets: has('v'),
            // V8's departures from the specification (vendor/regress/VENDORED.md).
            v8_compat: true,
        };
        let emitted = v8::emit(&parsed.tree, v8_flags);
        let regex = regress::Regex::from_unicode(emitted.into_iter(), compile_flags)
            .map_err(|e| format!("not modeled: regress refused /{pattern}/{ordered}: {e}"))?;
        // `RegExp::Compile` takes the atom path for a pattern that parses to a single atom,
        // unless the regex is sticky or ignores case or the atom has few different
        // characters; it then searches from `lastIndex` as it is, without stepping back
        // into a surrogate pair (node 22: `r = /😀/gu; r.lastIndex = 1; r.test('😀😀')`
        // leaves `lastIndex` at 4, `/😀b/gu` and `/(?:😀)/gu` at 2). Every other unicode
        // regex steps back with `g` or `y` (`OptionallyStepBackToLeadSurrogate`).
        let atom_path = parsed
            .atom
            .as_deref()
            .is_some_and(|atom| !has('i') && !has('y') && !v8::has_few_different_characters(atom));
        let steps_back = full_unicode && (has('g') || has('y')) && !atom_path;
        Ok(JsRegExp {
            pattern: pattern.to_string(),
            flags: ordered,
            global: has('g'),
            sticky: has('y'),
            steps_back,
            regex,
            last_index: AtomicUsize::new(0),
        })
    }

    /// `regexp.flags`.
    pub fn flags(&self) -> &str {
        &self.flags
    }

    /// The pattern as given to the constructor.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// `regexp.lastIndex`: where the next `test` of a `g` or `y` regex starts (always 0 for
    /// the others, which never move it).
    pub fn last_index(&self) -> usize {
        self.last_index.load(Ordering::Relaxed)
    }

    /// `regexp.test(s)` (RegExpBuiltinExec): with `g` or `y` the search starts at `lastIndex`
    /// and leaves it after the match (0 after a failure); with `y` the match must start there.
    /// `Err` where V8 throws: `RangeError: Maximum call stack size exceeded` when the match
    /// would need more backtracking than V8's stack holds (see [`BACKTRACK_LIMIT`]).
    pub fn test(&self, s: &str) -> Result<bool, String> {
        self.test_utf16(&s.encode_utf16().collect::<Vec<u16>>())
    }

    /// [`JsRegExp::test`] of a JS string value, code unit by code unit (a lone surrogate is
    /// that code unit, as the input JS gives the matcher).
    pub fn test_js(&self, s: &super::value::JsString) -> Result<bool, String> {
        self.test_utf16(&s.to_utf16())
    }

    /// [`JsRegExp::test`] of a string's UTF-16 code units.
    pub fn test_utf16(&self, text: &[u16]) -> Result<bool, String> {
        Ok(self.exec_utf16(text)?.is_some())
    }

    /// `regexp.exec(s)` of a string's UTF-16 code units (RegExpBuiltinExec, which `test` runs
    /// too): the match's code-unit range and each capture group's, or `None`. With `g` or `y`
    /// the search starts at `lastIndex` and leaves it after the match (0 after a failure);
    /// with `y` the match must start there. `Err` where V8 throws (as [`JsRegExp::test`]),
    /// `lastIndex` then unchanged.
    pub fn exec_utf16(&self, text: &[u16]) -> Result<Option<JsMatch>, String> {
        let stateful = self.global || self.sticky;
        let mut last_index = if stateful {
            self.last_index.load(Ordering::Relaxed)
        } else {
            0
        };
        if last_index > text.len() {
            if stateful {
                self.last_index.store(0, Ordering::Relaxed);
            }
            return Ok(None);
        }
        // With `u` or `v` the input is a list of code points, and the search starts at "the
        // character that was obtained from element lastIndex" (step 13.b): a `lastIndex` on
        // the trail surrogate of a pair names the pair's character, which starts one code unit
        // earlier. (V8 steps back to the lead surrogate the same way, except for an atom: see
        // `steps_back`.)
        let mut stepped_back = false;
        if self.steps_back
            && last_index > 0
            && last_index < text.len()
            && (0xDC00..=0xDFFF).contains(&text[last_index])
            && (0xD800..=0xDBFF).contains(&text[last_index - 1])
        {
            last_index -= 1;
            stepped_back = true;
        }
        // UTF-16 with `u` or `v`, else code units; with `y` only at `lastIndex`.
        let find = |at: usize| {
            self.regex
                .try_find_from_utf16(text, at, BACKTRACK_LIMIT, self.sticky)
                .map_err(|_| "RangeError: Maximum call stack size exceeded".to_string())
        };
        let mut found = find(last_index)?;
        if found.is_none() && stepped_back && self.sticky {
            // V8's step back is the first branch of a choice: when nothing matches at the
            // pair's start, a sticky regex is tried where `lastIndex` was, between the halves,
            // where only an empty match can be found (node 22: /\B/yu with lastIndex 2 finds
            // one at 2 in "a𝒳a").
            found = find(last_index + 1)?;
        }
        Ok(match found {
            Some(m) => {
                if stateful {
                    self.last_index.store(m.range.end, Ordering::Relaxed);
                }
                Some(JsMatch {
                    range: m.range,
                    captures: m.captures,
                })
            }
            None => {
                if stateful {
                    self.last_index.store(0, Ordering::Relaxed);
                }
                None
            }
        })
    }

    /// `regexp.source` (EscapeRegExpPattern, as V8 writes it): `/` outside a class escaped,
    /// line terminators written as escapes, `(?:)` for the empty pattern.
    pub fn source(&self) -> String {
        if self.pattern.is_empty() {
            return "(?:)".into();
        }
        let is_line_terminator = |c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
        let chars: Vec<char> = self.pattern.chars().collect();
        let mut out = String::new();
        let mut in_class = false;
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c == '\\' {
                if i + 1 < chars.len() && is_line_terminator(chars[i + 1]) {
                    // The backslash goes; the line terminator is escaped next.
                    i += 1;
                    continue;
                }
                // An escape: this and the next character as they are.
                out.push(c);
                i += 1;
                if i == chars.len() {
                    break;
                }
            } else if c == '/' && !in_class {
                out.push('\\');
            } else if c == '[' {
                in_class = true;
            } else if c == ']' {
                in_class = false;
            } else if c == '\n' {
                out.push_str("\\n");
                i += 1;
                continue;
            } else if c == '\r' {
                out.push_str("\\r");
                i += 1;
                continue;
            } else if c == '\u{2028}' {
                out.push_str("\\u2028");
                i += 1;
                continue;
            } else if c == '\u{2029}' {
                out.push_str("\\u2029");
                i += 1;
                continue;
            }
            out.push(chars[i]);
            i += 1;
        }
        out
    }

    /// `String(regexp)`: `/source/flags`.
    pub fn to_js_string(&self) -> String {
        format!("/{}/{}", self.source(), self.flags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_and_flags_as_js_writes_them() {
        // node: String(new RegExp('a/b[/]\\/c\n', 'yim')) === '/a\\/b[/]\\/c\\n/imy'
        let r = JsRegExp::new("a/b[/]\\/c\n", "yim").unwrap();
        assert_eq!(r.to_js_string(), "/a\\/b[/]\\/c\\n/imy");
        assert_eq!(JsRegExp::new("", "").unwrap().to_js_string(), "/(?:)/");
        assert!(JsRegExp::new("a", "gg").is_err());
        assert!(JsRegExp::new("a", "uv").is_err());
        assert!(JsRegExp::new("(", "").is_err());
    }

    #[test]
    fn regexps_print_as_node_prints_them() {
        // node: String(new RegExp(pattern, flags)), node 22.
        for (pattern, flags, printed) in [
            ("a/b[/]\\/c\n", "yim", "/a\\/b[/]\\/c\\n/imy"),
            ("", "", "/(?:)/"),
            ("\\\n", "g", "/\\n/g"),
            ("[\n/]", "s", "/[\\n/]/s"),
            ("\u{2028}x\u{2029}", "", "/\\u2028x\\u2029/"),
            ("\\/", "", "/\\//"),
            ("[\\]/]", "", "/[\\]/]/"),
            ("\r", "", "/\\r/"),
            ("(?:)", "d", "/(?:)/d"),
            ("\\\\/", "v", "/\\\\\\//v"),
        ] {
            let r = JsRegExp::new(pattern, flags).unwrap();
            assert_eq!(r.to_js_string(), printed, "{pattern:?} {flags:?}");
        }
    }

    #[test]
    fn test_keeps_last_index_for_global_and_sticky() {
        // node: const r = /a/g; [r.test('a'), r.test('a'), r.test('a')] -> [true, false, true]
        let r = JsRegExp::new("a", "g").unwrap();
        assert_eq!(
            (
                r.test("a").unwrap(),
                r.test("a").unwrap(),
                r.test("a").unwrap()
            ),
            (true, false, true)
        );
        // node: const y = /b/y; [y.test('ab'), y.test('b')] -> [false, true]
        let y = JsRegExp::new("b", "y").unwrap();
        assert_eq!((y.test("ab").unwrap(), y.test("b").unwrap()), (false, true));
        let plain = JsRegExp::new("^[a-z]+$", "i").unwrap();
        assert!(
            plain.test("AbC").unwrap() && plain.test("AbC").unwrap() && !plain.test("a1").unwrap()
        );
    }

    /// One regex, `test`ed on each string in turn: `(result, lastIndex)` after each.
    fn tests(pattern: &str, flags: &str, inputs: &[&str]) -> Vec<(bool, usize)> {
        let r = JsRegExp::new(pattern, flags).unwrap();
        inputs
            .iter()
            .map(|s| (r.test(s).unwrap(), r.last_index()))
            .collect()
    }

    #[test]
    fn a_last_index_inside_a_surrogate_pair_starts_at_the_pair() {
        // node 22: const r = new RegExp(pattern, flags);
        //   inputs.map(s => [r.test(s), r.lastIndex])
        const E: &str = "\u{1F600}"; // U+D83D U+DE00
        let xe = format!("x{E}");
        let ea = format!("{E}a");
        let eb = format!("{E}b");
        let ee = format!("{E}{E}");
        let xeb = format!("x{E}b");
        for flags in ["gu", "yu", "gv", "yv"] {
            assert_eq!(
                tests("^.", flags, &[&xe, &ea]),
                [(true, 1), (true, 2)],
                "/^./{flags}"
            );
        }
        // Without `u` the code units are the characters: no stepping back.
        for flags in ["g", "y"] {
            assert_eq!(
                tests("^.", flags, &[&xe, &ea]),
                [(true, 1), (false, 0)],
                "/^./{flags}"
            );
        }
        assert_eq!(
            tests(".", "gu", &["x", E, E, E]),
            [(true, 1), (true, 2), (false, 0), (true, 2)]
        );
        assert_eq!(
            tests(".", "yu", &["x", &ee, &ee, &ee]),
            [(true, 1), (true, 2), (true, 4), (false, 0)]
        );
        assert_eq!(
            tests("\\p{L}", "gu", &[&xeb, &eb, &eb, "b"]),
            [(true, 1), (true, 3), (false, 0), (true, 1)]
        );
        assert_eq!(
            tests("b", "yu", &["x", &eb, &eb]),
            [(false, 0), (false, 0), (false, 0)]
        );
        assert_eq!(
            tests("b", "gu", &["x", &eb, &eb]),
            [(false, 0), (true, 3), (false, 0)]
        );
        assert_eq!(tests(E, "yu", &["x", E]), [(false, 0), (true, 2)]);
        assert_eq!(tests("$", "gu", &["x", E]), [(true, 1), (true, 2)]);
        assert_eq!(tests("$", "yu", &["x", E]), [(false, 0), (false, 0)]);
        assert_eq!(
            tests("", "yu", &["x", E, E]),
            [(true, 0), (true, 0), (true, 0)]
        );
        assert_eq!(
            tests("\\b", "gu", &["x", &format!("{E}x")]),
            [(true, 0), (true, 2)]
        );
        assert_eq!(tests("\\uDE00", "gu", &["x", E]), [(false, 0), (false, 0)]);
        assert_eq!(
            tests("(?<=\\uD83D)", "gu", &["x", E]),
            [(false, 0), (false, 0)]
        );
        // Code units without `u`: a lone surrogate in the pattern matches half of the pair.
        assert_eq!(tests("\\uDE00", "g", &["x", E]), [(false, 0), (true, 2)]);
        assert_eq!(
            tests("(?<=\\uD83D)", "g", &["x", E]),
            [(false, 0), (true, 1)]
        );
        // A `lastIndex` past the end fails and goes back to 0.
        assert_eq!(tests("a", "gu", &["xxxxa", "a"]), [(true, 5), (false, 0)]);
        assert_eq!(tests("a", "yu", &["xxxxa", "a"]), [(false, 0), (true, 1)]);
        assert_eq!(tests(".", "gu", &["xxxx", E]), [(true, 1), (true, 2)]);
    }

    #[test]
    fn a_pattern_that_is_one_astral_character_does_not_step_back() {
        // V8 searches for an atom from `lastIndex` as it is (node 22, as above).
        const E: &str = "\u{1F600}";
        let xe = format!("x{E}");
        for (pattern, flags) in [
            (E, "gu"),
            (E, "gv"),
            (E, "gmsu"),
            ("\\u{1F600}", "gv"),
            ("\\u{01F600}", "gu"),
            ("\\uD83D\\uDE00", "gu"),
            ("\\uD83D\\uDE00", "gv"),
        ] {
            assert_eq!(
                tests(pattern, flags, &[E, &xe]),
                [(true, 2), (false, 0)],
                "/{pattern}/{flags}"
            );
        }
        // Not an atom (a group), sticky, or ignoring case: back to the pair's start.
        for (pattern, flags) in [
            ("(?:\u{1F600})", "gu"),
            (E, "yu"),
            (E, "giu"),
            ("\\u{1F600}", "gyu"),
        ] {
            assert_eq!(
                tests(pattern, flags, &[E, &xe]),
                [(true, 2), (true, 3)],
                "/{pattern}/{flags}"
            );
        }
    }

    #[test]
    fn classes_are_javascript_classes() {
        // \d is ASCII in JS; `.` stops at \r and U+2028 as well as \n.
        assert!(
            !JsRegExp::new("^\\d$", "")
                .unwrap()
                .test("\u{0663}")
                .unwrap()
        );
        assert!(
            !JsRegExp::new("^a.b$", "")
                .unwrap()
                .test("a\u{2028}b")
                .unwrap()
        );
        assert!(
            JsRegExp::new("^a.b$", "s")
                .unwrap()
                .test("a\u{2028}b")
                .unwrap()
        );
        // Without `u`, an astral character is two code units.
        assert!(
            JsRegExp::new("^..$", "")
                .unwrap()
                .test("\u{1F600}")
                .unwrap()
        );
        assert!(
            JsRegExp::new("^.$", "u")
                .unwrap()
                .test("\u{1F600}")
                .unwrap()
        );
    }

    /// `test` on each input in turn.
    fn results(pattern: &str, flags: &str, inputs: &[&str]) -> Vec<bool> {
        let r = JsRegExp::new(pattern, flags).expect("node compiles it");
        inputs.iter().map(|s| r.test(s).unwrap()).collect()
    }

    #[test]
    fn property_escapes_ignore_case_in_unicode_mode() {
        // node 22: `/\p{Lu}/iu.test('a')` is true (regress 0.11 said false; 0.12.0 agrees).
        assert_eq!(
            results("\\p{Lu}", "iu", &["a", "A", "1"]),
            [true, true, false]
        );
        assert_eq!(
            results("\\p{Lu}", "iv", &["a", "A", "1"]),
            [true, true, false]
        );
        assert_eq!(results("\\p{Ll}", "iu", &["A"]), [true]);
        assert_eq!(results("\\P{Lu}", "iu", &["a", "A"]), [true, true]);
    }

    #[test]
    fn a_nested_complement_of_nothing_in_v_mode() {
        // node 22: `/[[^]?]/v.test('a')` is true (regress 0.11 said false; 0.12.0 agrees).
        assert_eq!(results("[[^]?]", "v", &["a", "?", "/"]), [true, true, true]);
        assert_eq!(results("[[^]]", "v", &["a"]), [true]);
        assert_eq!(results("[^[^]]", "v", &["a"]), [false]);
    }

    /// Does `new RegExp(pattern, flags)` compile?
    fn compiles(pattern: &str, flags: &str) -> bool {
        JsRegExp::new(pattern, flags).is_ok()
    }

    #[test]
    fn surrogate_pair_escapes_are_two_code_units_without_u_or_v() {
        // node 22: new RegExp(pattern, flags), test() on each input in turn.
        const E: &str = "\u{1F600}"; // U+D83D U+DE00
        let xe = format!("x{E}");
        let ee = format!("{E}{E}");
        assert_eq!(
            results("\\uD83D\\uDE00", "", &[E, &xe, "x"]),
            [true, true, false]
        );
        assert_eq!(results("^\\uD83D\\uDE00$", "", &[E]), [true]);
        assert_eq!(results("^\\ud83d\\ude00$", "i", &[E]), [true]);
        assert_eq!(
            tests("\\uD83D\\uDE00", "g", &[E, &xe]),
            [(true, 2), (false, 0)]
        );
        // In a class they are two members, and no code unit is U+1F600.
        assert_eq!(results("^[\\uD83D\\uDE00]$", "", &[E, "a"]), [false, false]);
        assert_eq!(results("^[\\uD83D\\uDE00]{2}$", "", &[E]), [true]);
        assert_eq!(results("^[\\uD83D\\uDE00]+$", "", &[&ee]), [true]);
        assert_eq!(results("^[^\\uD83D\\uDE00]$", "", &[E, "a"]), [false, true]);
        // A range's end: [\uDBFF] and [\uDC00-\uDFFF], not U+10FC00 to U+DFFF.
        assert_eq!(
            results("^[\\uDBFF\\uDC00-\\uDFFF]{2}$", "", &[E, "\u{10FC00}"]),
            [false, true]
        );
        // Quantifiers, groups, lookaround and backreferences see two units.
        assert_eq!(results("^\\uD83D\\uDE00+$", "", &[E]), [true]);
        assert_eq!(results("^\\uD83D+\\uDE00$", "", &[E]), [true]);
        assert_eq!(results("^(\\uD83D)\\uDE00$", "", &[E]), [true]);
        assert_eq!(results("^\\uD83D(?=\\uDE00)", "", &[E]), [true]);
        assert_eq!(results("(?<=\\uD83D)\\uDE00", "", &[E]), [true]);
        assert_eq!(results("^(\\uD83D\\uDE00)\\1$", "", &[&ee]), [true]);
        // With `u` or `v` the pair is one code point; a group name's escapes are read so too.
        assert_eq!(results("^\\uD83D\\uDE00$", "u", &[E]), [true]);
        assert_eq!(results("^[\\uD83D\\uDE00]$", "u", &[E]), [true]);
        assert_eq!(results("^\\uD83D\\uDE00$", "v", &[E]), [true]);
        assert_eq!(
            results("(?<\\uD835\\uDC00>a)\\k<\\uD835\\uDC00>", "", &["aa"]),
            [true]
        );
    }

    #[test]
    fn a_lead_surrogate_escape_keeps_the_escape_after_it() {
        // node 22: `A` after `\uD83D` is `A`, `\u{1F600}` (with u) U+1F600.
        assert_eq!(
            results("^\\uD83D?\\u0041$", "", &["A", "0041"]),
            [true, false]
        );
        assert_eq!(
            results("^[\\uD83D\\u0041]$", "", &["A", "0"]),
            [true, false]
        );
        assert_eq!(
            results("^\\uD83D?\\u{1F600}$", "u", &["\u{1F600}", "u{1F600}"]),
            [true, false]
        );
        assert_eq!(
            results("^\\uD83D?\\u{61}$", "", &[&"u".repeat(61), "a"]),
            [true, false]
        );
    }

    #[test]
    fn a_braced_escape_without_u_is_u_repeated() {
        // node 22: Annex B reads `\u` as `u` and `{61}` as a quantifier.
        let u61 = "u".repeat(61);
        assert_eq!(results("^\\u{61}$", "", &["a", &u61]), [false, true]);
        assert_eq!(results("\\u{2028}?", "", &["a"]), [false]);
        assert_eq!(
            results("^\\u{1F600}$", "", &["\u{1F600}", "u{1F600}"]),
            [false, true]
        );
        assert_eq!(results("^[\\u{61}]+$", "", &["u{61}", "a"]), [true, false]);
        assert_eq!(results("^\\u$", "", &["u"]), [true]);
        assert_eq!(results("^\\u{61}$", "u", &["a"]), [true]);
        // `[\u{10000}-\u{10FFFF}]` is the range `}-u`: out of order.
        assert!(!compiles("[\\u{10000}-\\u{10FFFF}]", "g"));
    }

    #[test]
    fn v_is_a_unicode_mode() {
        // node 22: /k/iv matches U+212A as /k/iu does; `\a` and a quantified lookahead are
        // syntax errors with v as with u.
        assert_eq!(results("^k$", "iv", &["\u{212A}", "K"]), [true, true]);
        assert_eq!(results("K", "iv", &["\u{212A}"]), [true]);
        assert!(!compiles("\\a", "v"));
        assert!(!compiles("(?=a)*", "v"));
        assert!(!compiles("\\1", "v"));
        assert!(!compiles("\\q{abc}", "v"));
        assert!(compiles("[\\q{abc}]", "v"));
    }

    #[test]
    fn classes_ignoring_case_in_unicode_mode() {
        // node 22: \W in a class leaves out what folds into \w (ſ, U+212A) with i and u or v.
        assert_eq!(
            results("^[\\W]$", "iu", &["s", "\u{17F}", "k", "\u{212A}", "!"]),
            [false, false, false, false, true]
        );
        assert_eq!(
            results("^[^\\W]$", "iv", &["s", "\u{17F}", "\u{212A}", "!"]),
            [true, true, true, false]
        );
        assert_eq!(
            results("[^\\W]", "iu", &["\u{17F}", "\u{212A}", "s"]),
            [true, true, true]
        );
        assert_eq!(
            results("^[\\uD83D\\W]$", "iu", &["!", "s", "\u{17F}"]),
            [true, false, false]
        );
        // \P{...} with i and v: no letter is outside \p{Lu} once case is folded.
        assert_eq!(
            results("^\\P{Lu}$", "iv", &["a", "A", "1", "\u{17F}"]),
            [false, false, true, false]
        );
        // [\b] is U+0008 with v as without.
        assert_eq!(results("^[\\b]$", "v", &["b", "\u{8}"]), [false, true]);
        assert_eq!(
            results("^[\\uD83D\\b]$", "v", &["\u{8}", "b"]),
            [true, false]
        );
    }

    #[test]
    fn syntax_v8_rejects_is_a_syntax_error() {
        // node 22's V8 (12.4): "Nothing to repeat", "Invalid group" (modifiers are newer),
        // "Duplicate capture group name" (also across alternatives, newer too), "Invalid
        // escape" (`\k` in a class beside a named group).
        for (pattern, flags) in [
            ("\\b*", ""),
            ("\\b*", "u"),
            ("\\B{2}", ""),
            ("\\B{2}", "u"),
            ("\\B+?", "v"),
            ("\\b{1,}", ""),
            ("(?!a){2}", "v"),
            ("(?i:a)", ""),
            ("(?-i:a)", "u"),
            ("(?<n>a)(?<\\u006E>b)", ""),
            ("(?<a>x)|(?<a>y)", ""),
            ("[\\k](?<n>.)", ""),
        ] {
            assert!(!compiles(pattern, flags), "/{pattern}/{flags}");
        }
        // ... and what it accepts compiles.
        assert!(compiles("(?=a)*", ""));
        assert_eq!(results("\\b{a}", "", &["b{a}"]), [true]);
        assert_eq!(results("^[\\k]$", "", &["k"]), [true]);
        assert_eq!(results("^\\k<n>$", "", &["k<n>"]), [true]);
        assert_eq!(results("^(?<n>.)\\k<n>$", "", &["aa"]), [true]);
        assert_eq!(results("^\\uD83D?\\u{1F600}$", "u", &["\u{1F600}"]), [true]);
    }

    #[test]
    fn nested_classes_fit_a_small_stack() {
        // The parser recurses for each nested `v` class; at its limit it must fit a 2 MiB
        // thread in debug builds (the robustness policy's thread).
        let depth = v8::MAX_CLASS_NESTING;
        let worker = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || {
                let p = format!("{}a{}", "[".repeat(depth), "]".repeat(depth));
                let deep = JsRegExp::new(&p, "v");
                let p = format!("{}a{}", "[".repeat(depth + 1), "]".repeat(depth + 1));
                (deep.is_ok(), JsRegExp::new(&p, "v"))
            })
            .unwrap();
        let (_, past) = worker.join().unwrap();
        assert_eq!(
            past.unwrap_err(),
            format!(
                "SyntaxError: Invalid regular expression: /{}a{}/v: Maximum call stack size exceeded",
                "[".repeat(depth + 1),
                "]".repeat(depth + 1)
            )
        );
    }

    #[test]
    fn deep_groups_fit_a_small_stack() {
        let Ok(spec) = std::env::var("RX_PROBE") else {
            return;
        };
        let worker = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || {
                let mut it = spec.split(' ');
                let (open, mid, close, n) = (
                    it.next().unwrap().to_string(),
                    it.next().unwrap().to_string(),
                    it.next().unwrap().to_string(),
                    it.next().unwrap().parse::<usize>().unwrap(),
                );
                let p = format!("{}{}{}", open.repeat(n), mid, close.repeat(n));
                let t = std::time::Instant::now();
                let r = JsRegExp::new(&p, "");
                let out = match r {
                    Ok(r) => format!("ok {:?}", r.test("aaa")),
                    Err(e) => e
                        .chars()
                        .rev()
                        .take(50)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect(),
                };
                format!("{out} {:?}", t.elapsed())
            })
            .unwrap();
        let out = worker.join().unwrap();
        eprintln!("PROBE {out}");
    }

    #[test]
    fn every_property_name_v8_accepts_compiles() {
        // names.rs holds what node 22 accepts (tables/v8_names.cjs); each must reach regress
        // in a spelling it knows (with i too: the tables scan in work/regex checks that).
        for flags in ["u", "v"] {
            for (name, lone) in names::LONE.iter() {
                if matches!(lone, names::Lone::Strings(_)) && !flags.contains('v') {
                    continue;
                }
                for p in [format!("\\p{{{name}}}"), format!("[\\p{{{name}}}]")] {
                    assert!(JsRegExp::new(&p, flags).is_ok(), "/{p}/{flags}");
                }
                if !matches!(lone, names::Lone::Strings(_)) {
                    let p = format!("\\P{{{name}}}");
                    assert!(JsRegExp::new(&p, flags).is_ok(), "/{p}/{flags}");
                }
            }
            for key in ["gc", "General_Category"] {
                for (value, _) in names::GC_VALUES.iter() {
                    let p = format!("\\p{{{key}={value}}}");
                    assert!(JsRegExp::new(&p, flags).is_ok(), "/{p}/{flags}");
                }
            }
            for key in ["sc", "Script", "scx", "Script_Extensions"] {
                for (value, _) in names::SCRIPT_VALUES.iter() {
                    let p = format!("\\P{{{key}={value}}}");
                    assert!(JsRegExp::new(&p, flags).is_ok(), "/{p}/{flags}");
                }
            }
        }
    }

    #[test]
    fn syntax_errors_are_v8s() {
        // node 22: String(e) for `new RegExp(pattern, flags)` (probes/syntax_pins.json).
        for (pattern, flags, message) in [
            (
                "(",
                "",
                "SyntaxError: Invalid regular expression: /(/: Unterminated group",
            ),
            (
                "a)",
                "",
                "SyntaxError: Invalid regular expression: /a)/: Unmatched ')'",
            ),
            (
                "\\",
                "",
                "SyntaxError: Invalid regular expression: /\\/: \\ at end of pattern",
            ),
            (
                "\\p{Foo}",
                "u",
                "SyntaxError: Invalid regular expression: /\\p{Foo}/u: Invalid property name",
            ),
            (
                "[\\p{Foo}]",
                "u",
                "SyntaxError: Invalid regular expression: /[\\p{Foo}]/u: Invalid property name in character class",
            ),
            (
                "\\a",
                "u",
                "SyntaxError: Invalid regular expression: /\\a/u: Invalid escape",
            ),
            (
                "\\00",
                "u",
                "SyntaxError: Invalid regular expression: /\\00/u: Invalid decimal escape",
            ),
            (
                "\\u{110000}",
                "u",
                "SyntaxError: Invalid regular expression: /\\u{110000}/u: Invalid Unicode escape",
            ),
            (
                "*",
                "",
                "SyntaxError: Invalid regular expression: /*/: Nothing to repeat",
            ),
            (
                "a{2,1}",
                "",
                "SyntaxError: Invalid regular expression: /a{2,1}/: numbers out of order in {} quantifier",
            ),
            (
                "}",
                "u",
                "SyntaxError: Invalid regular expression: /}/u: Lone quantifier brackets",
            ),
            (
                "a{",
                "u",
                "SyntaxError: Invalid regular expression: /a{/u: Incomplete quantifier",
            ),
            (
                "(?=a)*",
                "u",
                "SyntaxError: Invalid regular expression: /(?=a)*/u: Invalid quantifier",
            ),
            (
                "(?<=a)*",
                "",
                "SyntaxError: Invalid regular expression: /(?<=a)*/: Invalid quantifier",
            ),
            (
                "(?i:a)",
                "",
                "SyntaxError: Invalid regular expression: /(?i:a)/: Invalid group",
            ),
            (
                "(?<1>a)",
                "",
                "SyntaxError: Invalid regular expression: /(?<1>a)/: Invalid capture group name",
            ),
            (
                "(?<a>x)(?<a>y)",
                "",
                "SyntaxError: Invalid regular expression: /(?<a>x)(?<a>y)/: Duplicate capture group name",
            ),
            (
                "\\k<a",
                "u",
                "SyntaxError: Invalid regular expression: /\\k<a/u: Invalid capture group name",
            ),
            (
                "\\k<b>(?<a>.)",
                "",
                "SyntaxError: Invalid regular expression: /\\k<b>(?<a>.)/: Invalid named capture referenced",
            ),
            (
                "[\\1]",
                "u",
                "SyntaxError: Invalid regular expression: /[\\1]/u: Invalid class escape",
            ),
            (
                "[\\d-a]",
                "u",
                "SyntaxError: Invalid regular expression: /[\\d-a]/u: Invalid character class",
            ),
            (
                "[a",
                "",
                "SyntaxError: Invalid regular expression: /[a/: Unterminated character class",
            ),
            (
                "[z-a]",
                "",
                "SyntaxError: Invalid regular expression: /[z-a]/: Range out of order in character class",
            ),
            (
                "[a&&&b]",
                "v",
                "SyntaxError: Invalid regular expression: /[a&&&b]/v: Invalid character in character class",
            ),
            (
                "[(]",
                "v",
                "SyntaxError: Invalid regular expression: /[(]/v: Invalid character in character class",
            ),
            (
                "[^\\q{ab}]",
                "v",
                "SyntaxError: Invalid regular expression: /[^\\q{ab}]/v: Negated character class may contain strings",
            ),
            (
                "[\\q{a]",
                "v",
                "SyntaxError: Invalid regular expression: /[\\q{a]/v: Invalid character in character class",
            ),
            (
                "[a--]",
                "v",
                "SyntaxError: Invalid regular expression: /[a--]/v: Invalid character in character class",
            ),
            (
                "[a-\\d]",
                "v",
                "SyntaxError: Invalid regular expression: /[a-\\d]/v: Invalid character class",
            ),
            (
                "\\P{RGI_Emoji}",
                "v",
                "SyntaxError: Invalid regular expression: /\\P{RGI_Emoji}/v: Invalid property name",
            ),
            (
                "\\p{RGI_Emoji}",
                "u",
                "SyntaxError: Invalid regular expression: /\\p{RGI_Emoji}/u: Invalid property name",
            ),
            (
                "[\\k](?<n>.)",
                "",
                "SyntaxError: Invalid regular expression: /[\\k](?<n>.)/: Invalid escape",
            ),
            (
                "(?<a>.)\\k",
                "",
                "SyntaxError: Invalid regular expression: /(?<a>.)\\k/: Invalid named reference",
            ),
            (
                "\\x1",
                "u",
                "SyntaxError: Invalid regular expression: /\\x1/u: Invalid escape",
            ),
            (
                "\\u12",
                "u",
                "SyntaxError: Invalid regular expression: /\\u12/u: Invalid Unicode escape",
            ),
            (
                "\\8",
                "u",
                "SyntaxError: Invalid regular expression: /\\8/u: Invalid escape",
            ),
            (
                "a**",
                "",
                "SyntaxError: Invalid regular expression: /a**/: Nothing to repeat",
            ),
            (
                "^*",
                "",
                "SyntaxError: Invalid regular expression: /^*/: Nothing to repeat",
            ),
            (
                "\\b+",
                "",
                "SyntaxError: Invalid regular expression: /\\b+/: Nothing to repeat",
            ),
            (
                "a|*",
                "",
                "SyntaxError: Invalid regular expression: /a|*/: Nothing to repeat",
            ),
            (
                "(?<a",
                "",
                "SyntaxError: Invalid regular expression: /(?<a/: Invalid capture group name",
            ),
            (
                "(?<\\u0031>.)",
                "",
                "SyntaxError: Invalid regular expression: /(?<\\u0031>.)/: Invalid capture group name",
            ),
            (
                "x{99999999999,1}",
                "",
                "SyntaxError: Invalid regular expression: /x{99999999999,1}/: numbers out of order in {} quantifier",
            ),
            (
                "[a-b--c]",
                "v",
                "SyntaxError: Invalid regular expression: /[a-b--c]/v: Invalid set operation in character class",
            ),
        ] {
            assert_eq!(
                JsRegExp::new(pattern, flags).unwrap_err(),
                message,
                "/{pattern}/{flags}"
            );
        }
        // ... and what V8 accepts compiles: Annex B (`\c` alone is a backslash and a `c`).
        assert_eq!(results("^\\c$", "", &["\\c"]), [true]);
        assert_eq!(results("^\\cx$", "u", &["\u{18}"]), [true]);
        assert!(compiles("[\\q{}]", ""));
    }

    #[test]
    fn a_match_past_v8s_backtrack_stack_throws() {
        // node 22: /(?:a|){3355442}/.exec("aa") matches, 3355443 throws RangeError.
        let throws = |n: usize| {
            JsRegExp::new(&format!("(?:a|){{{n}}}"), "")
                .unwrap()
                .test("aa")
                .is_err()
        };
        assert!(!throws(3_355_442));
        assert!(throws(3_355_443));
        let thrown = JsRegExp::new("(?:x?){3355443}", "").unwrap().test("xx");
        assert_eq!(
            thrown.unwrap_err(),
            "RangeError: Maximum call stack size exceeded"
        );
        // lastIndex stays where it was (node 22: r = /a|b(?:c|){4000000}/g; r.test("a") is
        // true with lastIndex 1, r.test("xb") throws and leaves it at 1).
        let r = JsRegExp::new("a|b(?:c|){4000000}", "g").unwrap();
        assert!(r.test("a").unwrap());
        assert_eq!(r.last_index(), 1);
        assert!(r.test("xb").is_err());
        assert_eq!(r.last_index(), 1);
    }

    #[test]
    fn sticky_retries_between_the_halves_of_a_pair() {
        // node 22: r = /aa|\B/yu; r.test("aa") leaves lastIndex 2; r.exec("a𝒳a") then
        // matches the empty string at 2, between the halves of 𝒳; /aa|(?:\B\S*\w*)/gyv too.
        let x: Vec<u16> = "a\u{1D4B3}a".encode_utf16().collect();
        for (pattern, flags) in [("aa|\\B", "yu"), ("aa|(?:\\B\\S*\\w*)", "gyv")] {
            let r = JsRegExp::new(pattern, flags).unwrap();
            assert!(r.test("aa").unwrap(), "/{pattern}/{flags}");
            assert_eq!(r.last_index(), 2);
            let m = r.exec_utf16(&x).unwrap();
            assert_eq!(m.unwrap().range, 2..2, "/{pattern}/{flags}");
            assert_eq!(r.last_index(), 2);
        }
    }

    #[test]
    fn sticky_tries_last_index_alone() {
        // node 22: /b/y and /(?:b|){3355443}c/y at lastIndex 0 of "ab"... fail at 0 only.
        assert_eq!(tests("b", "y", &["ab", "ab"]), [(false, 0), (false, 0)]);
        let r = JsRegExp::new("a(?:b|){3355443}|c", "y").unwrap();
        assert!(!r.test("xc").unwrap());
    }

    /// What V8 answers where the specification says otherwise or regress 0.12.0 did (each
    /// was a divergence of this module before the vendored fixes).
    mod v8_answers {
        use super::*;

        #[test]
        fn ignoring_case_without_u_keeps_non_ascii_apart_from_ascii() {
            assert_eq!(results("ſ", "i", &["s", "S", "ſ"]), [false, false, true]);
            assert_eq!(results("s", "i", &["ſ", "S"]), [false, true]);
            assert_eq!(results("ı", "i", &["i", "I", "ı"]), [false, false, true]);
            assert_eq!(results("i", "i", &["ı", "İ"]), [false, false]);
            assert_eq!(results("[a-z]", "i", &["ſ", "\u{212A}"]), [false, false]);
            assert_eq!(results("\\w", "i", &["ſ", "\u{212A}"]), [false, false]);
        }

        #[test]
        fn a_negated_property_in_a_class_ignoring_case_in_v_mode() {
            assert_eq!(
                results("^[\\P{Lu}]$", "iv", &["a", "A", "1"]),
                [false, false, true]
            );
            assert_eq!(
                results("^[^\\P{Lu}]$", "iv", &["a", "A", "1"]),
                [true, true, false]
            );
        }

        #[test]
        fn v8_does_not_fold_the_ascii_property_in_v_mode() {
            assert_eq!(
                results("^\\p{ASCII}$", "iv", &["s", "ſ", "k", "\u{212A}"]),
                [true, false, true, false]
            );
            assert_eq!(
                results("^\\P{ASCII}$", "iv", &["s", "ſ", "k", "\u{212A}"]),
                [false, true, false, true]
            );
            // node 22: a one-character \q string is its case folding alone.
            assert_eq!(
                results("^[\\q{K}]$", "iv", &["K", "k", "\u{212A}"]),
                [false, true, false]
            );
        }

        #[test]
        fn v8_matches_an_empty_string_inside_a_surrogate_pair() {
            let e = "\u{1F600}";
            let s = format!("a{e}xab");
            assert_eq!(tests("\\B", "gu", &[&s]), [(true, 2)]);
            assert_eq!(tests("\\B", "gv", &[&s]), [(true, 2)]);
            assert_eq!(tests("(?!\\u{1F600})", "gu", &[e]), [(true, 1)]);
            // Without g too: /(?!\p{Any})(?!$)/u matches "😀" at 1.
            let r = JsRegExp::new("(?!\\p{Any})(?!$)", "u").unwrap();
            let m = r
                .exec_utf16(&e.encode_utf16().collect::<Vec<u16>>())
                .unwrap();
            assert_eq!(m.unwrap().range, 1..1);
        }

        #[test]
        fn v8_folds_backreferences_fully_in_a_two_byte_subject() {
            // node 22: /(ﬀf)\1/iu.exec("ﬀffﬀ") matches; /(ßs)\1/iu.exec("ßssß") does not
            // (V8 stores that subject one byte a unit), with "Ā" after it it does.
            assert_eq!(results("(ﬀf)\\1", "iu", &["ﬀffﬀ"]), [true]);
            assert_eq!(results("(ßs)\\1", "iu", &["ßssß", "ßssßĀ"]), [false, true]);
            assert_eq!(results("(ßs)\\1", "i", &["ßssßĀ"]), [false]);
        }
    }
}
