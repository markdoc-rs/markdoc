//! V8's regular-expression parser, ported: node 22.22.1's V8 12.4 `src/regexp/regexp-parser.cc`,
//! with its flags as node runs it (`js_regexp_modifiers` and
//! `js_regexp_duplicate_named_groups` off).
//!
//! It answers what `new RegExp(pattern, flags)` answers: whether the pattern compiles and, when
//! it does not, V8's `SyntaxError` message, found where V8 finds it (the first error in V8's
//! parse order). When it compiles, it gives the pattern as V8 reads it, as a tree that
//! [`emit`] writes out in a canonical spelling regress reads the same way (see the module
//! `super`), and what V8's `RegExp::Compile` decides from the parse (a single atom, which V8
//! searches for without stepping back into a surrogate pair).
//!
//! The functions keep V8's names and order (`ParseDisjunction` is [`Parser::disjunction`],
//! ...), so each can be checked against the original line by line.

use super::names::{self, Lone};

/// `kEndMarker`: past the last character.
const END: u32 = 1 << 21;
/// `RegExpTree::kInfinity`.
pub(super) const INFINITY: u32 = i32::MAX as u32;
/// `RegExpMacroAssembler::kMaxCaptures`.
const MAX_CAPTURES: usize = ((1 << 16) - 1 - 1) / 2;
/// How deep v-mode classes may nest. V8 recurses for each nested class and fails with its
/// stack (node 22: `SyntaxError: ... Maximum call stack size exceeded` from 6,944 levels);
/// the port's parser recurses too and stops here, where a 2 MiB thread still has room in a
/// debug build.
pub(super) const MAX_CLASS_NESTING: usize = 1_000;

/// A `RegExpError` the parser reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    StackOverflow,
    UnterminatedGroup,
    UnmatchedParen,
    EscapeAtEndOfPattern,
    InvalidPropertyName,
    InvalidEscape,
    InvalidDecimalEscape,
    InvalidUnicodeEscape,
    NothingToRepeat,
    LoneQuantifierBrackets,
    RangeOutOfOrder,
    IncompleteQuantifier,
    InvalidQuantifier,
    InvalidGroup,
    TooManyCaptures,
    InvalidCaptureGroupName,
    DuplicateCaptureGroupName,
    InvalidNamedReference,
    InvalidNamedCaptureReference,
    InvalidClassEscape,
    InvalidClassPropertyName,
    InvalidCharacterClass,
    UnterminatedCharacterClass,
    OutOfOrderCharacterClass,
    InvalidClassSetOperation,
    InvalidCharacterInClass,
    NegatedCharacterClassWithStrings,
}

impl Error {
    /// V8's message (`RegExpErrorString`).
    pub(super) fn message(self) -> &'static str {
        match self {
            Error::StackOverflow => "Maximum call stack size exceeded",
            Error::UnterminatedGroup => "Unterminated group",
            Error::UnmatchedParen => "Unmatched ')'",
            Error::EscapeAtEndOfPattern => "\\ at end of pattern",
            Error::InvalidPropertyName => "Invalid property name",
            Error::InvalidEscape => "Invalid escape",
            Error::InvalidDecimalEscape => "Invalid decimal escape",
            Error::InvalidUnicodeEscape => "Invalid Unicode escape",
            Error::NothingToRepeat => "Nothing to repeat",
            Error::LoneQuantifierBrackets => "Lone quantifier brackets",
            Error::RangeOutOfOrder => "numbers out of order in {} quantifier",
            Error::IncompleteQuantifier => "Incomplete quantifier",
            Error::InvalidQuantifier => "Invalid quantifier",
            Error::InvalidGroup => "Invalid group",
            Error::TooManyCaptures => "Too many captures",
            Error::InvalidCaptureGroupName => "Invalid capture group name",
            Error::DuplicateCaptureGroupName => "Duplicate capture group name",
            Error::InvalidNamedReference => "Invalid named reference",
            Error::InvalidNamedCaptureReference => "Invalid named capture referenced",
            Error::InvalidClassEscape => "Invalid class escape",
            Error::InvalidClassPropertyName => "Invalid property name in character class",
            Error::InvalidCharacterClass => "Invalid character class",
            Error::UnterminatedCharacterClass => "Unterminated character class",
            Error::OutOfOrderCharacterClass => "Range out of order in character class",
            Error::InvalidClassSetOperation => "Invalid set operation in character class",
            Error::InvalidCharacterInClass => "Invalid character in character class",
            Error::NegatedCharacterClassWithStrings => {
                "Negated character class may contain strings"
            }
        }
    }
}

/// The flags the parser reads.
#[derive(Clone, Copy, Debug)]
pub(super) struct Flags {
    pub(super) unicode: bool,
    pub(super) sets: bool,
}

/// A Unicode property, by the spelling regress is given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Property {
    Lone(Lone),
    Gc(&'static str),
    Script(&'static str),
    ScriptExtensions(&'static str),
}

/// A class escape `\d`, `\D`, `\s`, `\S`, `\w` or `\W`, by its letter.
pub(super) type Escape = u8;

/// One member of a class without `v` (`ParseClassRanges`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ClassItem {
    Range(u32, u32),
    Escape(Escape),
    Property(bool, Property),
}

/// How the operands of a `v` class combine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SetOp {
    Union,
    Intersection,
    Subtraction,
}

/// A `v` class (`RegExpClassSetExpression`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SetExpr {
    pub(super) negated: bool,
    pub(super) op: SetOp,
    pub(super) operands: Vec<SetOperand>,
}

/// An operand of a `v` class. A union's characters and ranges are kept in order with its
/// other operands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SetOperand {
    Range(u32, u32),
    Escape(Escape),
    Property(bool, Property),
    /// `\q{...}`: its strings, each a list of code points (possibly empty).
    Strings(Vec<Vec<u32>>),
    Nested(SetExpr),
}

/// An assertion (`RegExpAssertion`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Assertion {
    Start,
    End,
    Boundary,
    NonBoundary,
}

/// A term of the parsed pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Term {
    /// A character: a code unit without `u` or `v`, a code point with either (a surrogate is
    /// a lone one).
    Char(u32),
    Dot,
    Class(bool, Vec<ClassItem>),
    SetClass(SetExpr),
    /// `\p{...}` or `\P{...}` outside a class.
    Property(bool, Property),
    /// `\d` ... `\W` outside a class.
    Escape(Escape),
    Assertion(Assertion),
    /// A backreference by group number (1-based), or by name until the end of the parse.
    BackRef(usize),
    NamedBackRef(Vec<u16>),
    Group(Disjunction),
    Capture(usize, Disjunction),
    Lookaround {
        ahead: bool,
        positive: bool,
        body: Disjunction,
    },
    Quantified {
        body: Box<Term>,
        min: u32,
        max: u32,
        greedy: bool,
    },
}

/// Alternatives, each a list of terms.
pub(super) type Disjunction = Vec<Vec<Term>>;

/// What V8 makes of a pattern that parses.
#[derive(Debug)]
pub(super) struct Parsed {
    pub(super) tree: Disjunction,
    /// The tree is a single `RegExpAtom`: its code units.
    pub(super) atom: Option<Vec<u16>>,
}

/// `RegExpTree::max_match() == 0`: a term that can only match the empty string, as V8
/// computes it (`sets`: the `v` flag, where a class or a property escape is a
/// `RegExpClassSetOperand`, whose `max_match` follows its contents).
fn matches_nothing_but_empty(term: &Term, sets: bool) -> bool {
    match term {
        Term::Assertion(_) | Term::Lookaround { .. } => true,
        Term::Group(body) | Term::Capture(_, body) => body.iter().all(|alternative| {
            alternative
                .iter()
                .all(|t| matches_nothing_but_empty(t, sets))
        }),
        Term::Quantified { body, max, .. } => *max == 0 || matches_nothing_but_empty(body, sets),
        Term::SetClass(expr) => set_max_match_is_zero(expr),
        Term::Property(negated, property) => sets && property_is_empty(*negated, property),
        _ => false,
    }
}

/// `RegExpClassSetExpression::max_match() == 0`: not negated, and every operand matches
/// nothing but the empty string (an empty class, `\P{Any}`, `\q{}`).
fn set_max_match_is_zero(expr: &SetExpr) -> bool {
    !expr.negated
        && expr.operands.iter().all(|operand| match operand {
            SetOperand::Range(..) | SetOperand::Escape(_) => false,
            SetOperand::Property(negated, property) => property_is_empty(*negated, property),
            SetOperand::Strings(strings) => strings.iter().all(|s| s.is_empty()),
            SetOperand::Nested(expr) => set_max_match_is_zero(expr),
        })
}

/// A property escape whose set is empty: `\P{Any}` (V8's other properties hold something).
fn property_is_empty(negated: bool, property: &Property) -> bool {
    negated && *property == Property::Lone(Lone::Any)
}

/// `RegExpBuilder`: the terms of the current alternative and the alternatives so far.
#[derive(Default)]
struct Builder {
    pending_empty: bool,
    terms: Vec<Term>,
    alternatives: Vec<Vec<Term>>,
}

impl Builder {
    fn add(&mut self, term: Term) {
        self.pending_empty = false;
        self.terms.push(term);
    }

    fn add_empty(&mut self) {
        self.pending_empty = true;
    }

    fn new_alternative(&mut self) {
        let terms = std::mem::take(&mut self.terms);
        self.alternatives.push(terms);
    }

    fn finish(mut self) -> Disjunction {
        self.new_alternative();
        self.alternatives
    }

    /// `AddQuantifierToAtom`: `false` where V8 reports `kInvalidQuantifier`.
    fn quantify(
        &mut self,
        min: u32,
        max: u32,
        greedy: bool,
        unicode_mode: bool,
        sets: bool,
    ) -> bool {
        if self.pending_empty {
            self.pending_empty = false;
            return true;
        }
        let Some(atom) = self.terms.pop() else {
            // V8: UNREACHABLE (only called after an atom).
            return true;
        };
        if let Term::Lookaround { ahead, .. } = atom {
            // With /u or /v, lookarounds are not quantifiable; lookbehinds never are.
            if unicode_mode || !ahead {
                return false;
            }
        }
        if matches_nothing_but_empty(&atom, sets) {
            // V8 keeps such a term once, or drops it when it may be repeated zero times.
            // A dropped term keeps its capture groups, which never take part: it is written
            // out repeated zero times.
            if min != 0 {
                self.terms.push(atom);
            } else {
                self.terms.push(Term::Quantified {
                    body: Box::new(atom),
                    min: 0,
                    max: 0,
                    greedy: true,
                });
            }
            return true;
        }
        self.terms.push(Term::Quantified {
            body: Box::new(atom),
            min,
            max,
            greedy,
        });
        true
    }
}

/// What a parenthesized group is (`SubexpressionType`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum GroupType {
    Initial,
    Capture,
    PositiveLookaround,
    NegativeLookaround,
    Grouping,
}

/// `RegExpParserState`.
struct State {
    builder: Builder,
    group_type: GroupType,
    lookahead: bool,
    capture_index: usize,
    capture_name: Option<Vec<u16>>,
}

/// Where an escape is parsed (`InClassEscapeState`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum InClass {
    Yes,
    No,
}

/// What `ParseClassSetOperand` found (`ClassSetOperandType`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum OperandType {
    ClassSetCharacter,
    ClassStringDisjunction,
    NestedClass,
    CharacterClassEscape,
    ClassSetRange,
}

/// `RegExpParserImpl<base::uc16>`.
struct Parser<'a> {
    input: &'a [u16],
    current: u32,
    next_pos: usize,
    has_more: bool,
    flags: Flags,
    force_unicode: bool,
    error: Option<Error>,
    captures_started: usize,
    capture_count: usize,
    is_scanned_for_captures: bool,
    has_named_captures: bool,
    /// Capture names and their group numbers.
    named_captures: Vec<(Vec<u16>, usize)>,
    named_back_references: Vec<Vec<u16>>,
    class_nesting: usize,
}

/// Parses `pattern` (UTF-16 code units) with `flags` as V8 does.
pub(super) fn parse(pattern: &[u16], flags: Flags) -> Result<Parsed, Error> {
    let mut p = Parser {
        input: pattern,
        current: END,
        next_pos: 0,
        has_more: true,
        flags,
        force_unicode: false,
        error: None,
        captures_started: 0,
        capture_count: 0,
        is_scanned_for_captures: false,
        has_named_captures: false,
        named_captures: Vec::new(),
        named_back_references: Vec::new(),
        class_nesting: 0,
    };
    p.advance();
    let tree = p.disjunction();
    if let Some(e) = p.error {
        return Err(e);
    }
    let mut tree = tree.expect("a parse without an error has a tree");
    p.patch_named_back_references(&mut tree)?;
    let atom = single_atom(&tree, flags.unicode || flags.sets);
    Ok(Parsed { tree, atom })
}

/// The code units of the tree when it is a single `RegExpAtom`: one alternative of plain
/// characters that V8's text builder keeps in one atom (BMP characters that are no
/// surrogates, or, with `u` or `v`, exactly one character outside the BMP, which is an atom
/// of its own).
fn single_atom(tree: &Disjunction, unicode_mode: bool) -> Option<Vec<u16>> {
    let [terms] = tree.as_slice() else {
        return None;
    };
    if terms.is_empty() {
        return None;
    }
    let mut units = Vec::new();
    for term in terms {
        let Term::Char(c) = *term else {
            return None;
        };
        if unicode_mode && (0xD800..=0xDFFF).contains(&c) {
            return None;
        }
        if c > 0xFFFF {
            if terms.len() != 1 {
                return None;
            }
            let c = c - 0x10000;
            units.push(0xD800 + (c >> 10) as u16);
            units.push(0xDC00 + (c & 0x3FF) as u16);
        } else {
            units.push(c as u16);
        }
    }
    Some(units)
}

fn is_lead(c: u32) -> bool {
    (0xD800..=0xDBFF).contains(&c)
}

fn is_trail(c: u32) -> bool {
    (0xDC00..=0xDFFF).contains(&c)
}

fn is_decimal_digit(c: u32) -> bool {
    (u32::from(b'0')..=u32::from(b'9')).contains(&c)
}

fn hex_value(c: u32) -> Option<u32> {
    char::from_u32(c).and_then(|c| c.to_digit(16))
}

/// `IsSyntaxCharacterOrSlash`.
fn is_syntax_character_or_slash(c: u32) -> bool {
    matches!(
        char::from_u32(c),
        Some(
            '^' | '$'
                | '\\'
                | '.'
                | '*'
                | '+'
                | '?'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '|'
                | '/'
        )
    )
}

/// `IsClassSetSyntaxCharacter`.
fn is_class_set_syntax_character(c: u32) -> bool {
    matches!(
        char::from_u32(c),
        Some('(' | ')' | '[' | ']' | '{' | '}' | '/' | '-' | '\\' | '|')
    )
}

/// `IsClassSetReservedPunctuator`.
fn is_class_set_reserved_punctuator(c: u32) -> bool {
    matches!(
        char::from_u32(c),
        Some('&' | '-' | '!' | '#' | '%' | ',' | ':' | ';' | '<' | '=' | '>' | '@' | '`' | '~')
    )
}

/// The characters `IsClassSetReservedDoublePunctuator` doubles.
fn is_double_punctuator_char(c: u32) -> bool {
    matches!(
        char::from_u32(c),
        Some(
            '&' | '!'
                | '#'
                | '$'
                | '%'
                | '*'
                | '+'
                | ','
                | '.'
                | ':'
                | ';'
                | '<'
                | '='
                | '>'
                | '?'
                | '@'
                | '^'
                | '`'
                | '~'
        )
    )
}

/// `IsUnicodePropertyValueCharacter`.
fn is_property_value_character(c: u32) -> bool {
    char::from_u32(c).is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl Parser<'_> {
    fn unicode_mode(&self) -> bool {
        self.flags.unicode || self.flags.sets || self.force_unicode
    }

    fn failed(&self) -> bool {
        self.error.is_some()
    }

    fn has_next(&self) -> bool {
        self.next_pos < self.input.len()
    }

    /// `ReadNext`: the character at `next_pos` (a surrogate pair in unicode mode).
    fn read_next(&mut self, update_position: bool) -> u32 {
        let mut position = self.next_pos;
        let c0 = u32::from(self.input[position]);
        let mut result = c0;
        position += 1;
        if self.unicode_mode() && position < self.input.len() && is_lead(c0) {
            let c1 = u32::from(self.input[position]);
            if is_trail(c1) {
                result = 0x10000 + ((c0 - 0xD800) << 10) + (c1 - 0xDC00);
                position += 1;
            }
        }
        if update_position {
            self.next_pos = position;
        }
        result
    }

    /// `Next`.
    fn next(&mut self) -> u32 {
        if self.has_next() {
            self.read_next(false)
        } else {
            END
        }
    }

    /// `Advance()`.
    fn advance(&mut self) {
        if self.has_next() {
            self.current = self.read_next(true);
        } else {
            self.current = END;
            self.next_pos = self.input.len() + 1;
            self.has_more = false;
        }
    }

    /// `Advance(dist)`.
    fn advance_by(&mut self, dist: isize) {
        self.next_pos = (self.next_pos as isize + dist - 1) as usize;
        self.advance();
    }

    /// `RewindByOneCodepoint`.
    fn rewind_by_one_codepoint(&mut self) {
        if !self.has_more {
            return;
        }
        let rewind_by = if self.current > 0xFFFF { -2 } else { -1 };
        self.advance_by(rewind_by);
    }

    /// `Reset`.
    fn reset(&mut self, pos: usize) {
        self.next_pos = pos;
        self.has_more = pos < self.input.len();
        self.advance();
    }

    /// `position()`.
    fn position(&self) -> usize {
        self.next_pos - 1
    }

    /// `ReportError`: the first error stays.
    fn report(&mut self, error: Error) {
        if self.failed() {
            return;
        }
        self.error = Some(error);
        self.current = END;
        self.next_pos = self.input.len();
        self.has_more = false;
    }

    /// `ParseDisjunction`, with the state stack V8 keeps for groups.
    fn disjunction(&mut self) -> Option<Disjunction> {
        let mut stack: Vec<State> = Vec::new();
        let mut state = State {
            builder: Builder::default(),
            group_type: GroupType::Initial,
            lookahead: true,
            capture_index: 0,
            capture_name: None,
        };
        loop {
            match self.current {
                END => {
                    if self.failed() {
                        return None;
                    }
                    if !stack.is_empty() {
                        self.report(Error::UnterminatedGroup);
                        return None;
                    }
                    return Some(state.builder.finish());
                }
                c if c == u32::from(b')') => {
                    if stack.is_empty() {
                        self.report(Error::UnmatchedParen);
                        return None;
                    }
                    self.advance();
                    let finished = std::mem::replace(&mut state, stack.pop().expect("a group"));
                    let body = finished.builder.finish();
                    let term = match finished.group_type {
                        GroupType::Capture => {
                            if let Some(name) = finished.capture_name {
                                // CreateNamedCaptureAtIndex.
                                if self.named_captures.iter().any(|(n, _)| *n == name) {
                                    self.report(Error::DuplicateCaptureGroupName);
                                    return None;
                                }
                                self.named_captures.push((name, finished.capture_index));
                            }
                            Term::Capture(finished.capture_index, body)
                        }
                        GroupType::Grouping => Term::Group(body),
                        GroupType::PositiveLookaround | GroupType::NegativeLookaround => {
                            Term::Lookaround {
                                ahead: finished.lookahead,
                                positive: finished.group_type == GroupType::PositiveLookaround,
                                body,
                            }
                        }
                        GroupType::Initial => unreachable!("the initial state is not a group"),
                    };
                    state.builder.add(term);
                    // Quantifiers may follow.
                }
                c if c == u32::from(b'|') => {
                    self.advance();
                    state.builder.new_alternative();
                    continue;
                }
                c if c == u32::from(b'*') || c == u32::from(b'+') || c == u32::from(b'?') => {
                    self.report(Error::NothingToRepeat);
                    return None;
                }
                c if c == u32::from(b'^') => {
                    self.advance();
                    state.builder.add(Term::Assertion(Assertion::Start));
                    continue;
                }
                c if c == u32::from(b'$') => {
                    self.advance();
                    state.builder.add(Term::Assertion(Assertion::End));
                    continue;
                }
                c if c == u32::from(b'.') => {
                    self.advance();
                    state.builder.add(Term::Dot);
                }
                c if c == u32::from(b'(') => {
                    let new_state = self.open_parenthesis(&state)?;
                    stack.push(std::mem::replace(&mut state, new_state));
                    continue;
                }
                c if c == u32::from(b'[') => {
                    let class = self.character_class()?;
                    state.builder.add(class);
                }
                c if c == u32::from(b'\\') => match self.next() {
                    END => {
                        self.report(Error::EscapeAtEndOfPattern);
                        return None;
                    }
                    n if (u32::from(b'1')..=u32::from(b'9')).contains(&n) => {
                        let index = self.back_reference_index();
                        if self.failed() {
                            return None;
                        }
                        if let Some(index) = index {
                            if inside_capture_group(&state, &stack, index) {
                                // Nothing can have been captured yet: empty.
                                state.builder.add_empty();
                            } else {
                                state.builder.add(Term::BackRef(index));
                            }
                        } else {
                            // With /u and /v, no identity escapes except for syntax
                            // characters are allowed. Otherwise, all are.
                            if self.unicode_mode() {
                                self.report(Error::InvalidEscape);
                                return None;
                            }
                            let first_digit = self.next();
                            if first_digit == u32::from(b'8') || first_digit == u32::from(b'9') {
                                state.builder.add(Term::Char(first_digit));
                                self.advance_by(2);
                            } else {
                                // Octal, as `\0` below.
                                self.advance();
                                let octal = self.octal_literal();
                                state.builder.add(Term::Char(octal));
                            }
                        }
                    }
                    n if n == u32::from(b'0') => {
                        self.advance();
                        if self.unicode_mode() && is_decimal_digit(self.next()) {
                            // Decimal escapes with a leading 0 are not octal.
                            self.report(Error::InvalidDecimalEscape);
                            return None;
                        }
                        let octal = self.octal_literal();
                        state.builder.add(Term::Char(octal));
                    }
                    n if n == u32::from(b'b') || n == u32::from(b'B') => {
                        self.advance_by(2);
                        state.builder.add(Term::Assertion(if n == u32::from(b'b') {
                            Assertion::Boundary
                        } else {
                            Assertion::NonBoundary
                        }));
                        continue;
                    }
                    n if b"dDsSwW".contains(&(n as u8)) && n < 0x80 => {
                        self.advance_by(2);
                        state.builder.add(Term::Escape(n as u8));
                    }
                    n if n == u32::from(b'p') || n == u32::from(b'P') => {
                        match self.character_class_escape_property(n, InClass::No) {
                            Some((negated, property)) => {
                                state.builder.add(Term::Property(negated, property));
                            }
                            None => {
                                if self.failed() {
                                    return None;
                                }
                                // Not unicode: an identity escape.
                                self.advance_by(2);
                                state.builder.add(Term::Char(n));
                            }
                        }
                    }
                    n if n == u32::from(b'k') => {
                        // An identity escape for non-unicode patterns without named captures,
                        // else a named backreference.
                        let has_named_captures = self.has_named_captures(InClass::No);
                        if self.failed() {
                            return None;
                        }
                        if self.unicode_mode() || has_named_captures {
                            self.advance_by(2);
                            self.named_back_reference(&mut state, &stack);
                            if self.failed() {
                                return None;
                            }
                        } else {
                            let (c, _) = self.character_escape(InClass::No);
                            if self.failed() {
                                return None;
                            }
                            state.builder.add(Term::Char(c));
                        }
                    }
                    _ => {
                        let (c, _escaped_unicode) = self.character_escape(InClass::No);
                        if self.failed() {
                            return None;
                        }
                        state.builder.add(Term::Char(c));
                    }
                },
                c if c == u32::from(b'{') => {
                    if self.interval_quantifier().is_some() {
                        self.report(Error::NothingToRepeat);
                        return None;
                    }
                    if self.unicode_mode() {
                        self.report(Error::LoneQuantifierBrackets);
                        return None;
                    }
                    state.builder.add(Term::Char(c));
                    self.advance();
                }
                c if c == u32::from(b'}') || c == u32::from(b']') => {
                    if self.unicode_mode() {
                        self.report(Error::LoneQuantifierBrackets);
                        return None;
                    }
                    state.builder.add(Term::Char(c));
                    self.advance();
                }
                c => {
                    state.builder.add(Term::Char(c));
                    self.advance();
                }
            }

            // A quantifier may follow the atom just added.
            let (min, max) = match self.current {
                c if c == u32::from(b'*') => {
                    self.advance();
                    (0, INFINITY)
                }
                c if c == u32::from(b'+') => {
                    self.advance();
                    (1, INFINITY)
                }
                c if c == u32::from(b'?') => {
                    self.advance();
                    (0, 1)
                }
                c if c == u32::from(b'{') => match self.interval_quantifier() {
                    Some((min, max)) => {
                        if max < min {
                            self.report(Error::RangeOutOfOrder);
                            return None;
                        }
                        (min, max)
                    }
                    None => {
                        if self.unicode_mode() {
                            self.report(Error::IncompleteQuantifier);
                            return None;
                        }
                        continue;
                    }
                },
                _ => continue,
            };
            let mut greedy = true;
            if self.current == u32::from(b'?') {
                greedy = false;
                self.advance();
            }
            let unicode_mode = self.unicode_mode();
            let sets = self.flags.sets;
            if !state.builder.quantify(min, max, greedy, unicode_mode, sets) {
                self.report(Error::InvalidQuantifier);
                return None;
            }
        }
    }

    /// `ParseOpenParenthesis`.
    fn open_parenthesis(&mut self, state: &State) -> Option<State> {
        let mut lookahead = state.lookahead;
        let mut is_named_capture = false;
        let mut group_type = GroupType::Capture;
        self.advance();
        if self.current == u32::from(b'?') {
            match self.next() {
                // Modifiers (`(?i:`) are behind a flag node 22 leaves off.
                c if c == u32::from(b'-')
                    || c == u32::from(b'm')
                    || c == u32::from(b'i')
                    || c == u32::from(b's') =>
                {
                    self.report(Error::InvalidGroup);
                    return None;
                }
                c if c == u32::from(b':') => {
                    self.advance_by(2);
                    group_type = GroupType::Grouping;
                }
                c if c == u32::from(b'=') => {
                    self.advance_by(2);
                    lookahead = true;
                    group_type = GroupType::PositiveLookaround;
                }
                c if c == u32::from(b'!') => {
                    self.advance_by(2);
                    lookahead = true;
                    group_type = GroupType::NegativeLookaround;
                }
                c if c == u32::from(b'<') => {
                    self.advance();
                    if self.next() == u32::from(b'=') {
                        self.advance_by(2);
                        lookahead = false;
                        group_type = GroupType::PositiveLookaround;
                    } else if self.next() == u32::from(b'!') {
                        self.advance_by(2);
                        lookahead = false;
                        group_type = GroupType::NegativeLookaround;
                    } else {
                        is_named_capture = true;
                        self.has_named_captures = true;
                        self.advance();
                    }
                }
                _ => {
                    self.report(Error::InvalidGroup);
                    return None;
                }
            }
        }
        let mut capture_name = None;
        if group_type == GroupType::Capture {
            if self.captures_started >= MAX_CAPTURES {
                self.report(Error::TooManyCaptures);
                return None;
            }
            self.captures_started += 1;
            if is_named_capture {
                capture_name = Some(self.capture_group_name()?);
            }
        }
        Some(State {
            builder: Builder::default(),
            group_type,
            lookahead,
            capture_index: self.captures_started,
            capture_name,
        })
    }

    /// `ScanForCaptures`: counts every capture group, so that `\N` knows whether it is a
    /// backreference, and notes whether there are named ones.
    fn scan_for_captures(&mut self, in_class: InClass) {
        let saved_position = self.position();
        let mut capture_count = self.captures_started;
        if in_class == InClass::Yes {
            loop {
                let c = self.current;
                if c == END {
                    break;
                }
                self.advance();
                if c == u32::from(b'\\') {
                    self.advance();
                } else if c == u32::from(b']') {
                    break;
                }
            }
        }
        loop {
            let n = self.current;
            if n == END {
                break;
            }
            self.advance();
            if n == u32::from(b'\\') {
                self.advance();
            } else if n == u32::from(b'[') {
                let mut class_nest_level = 0usize;
                loop {
                    let c = self.current;
                    if c == END {
                        break;
                    }
                    self.advance();
                    if c == u32::from(b'\\') {
                        self.advance();
                    } else if c == u32::from(b'[') {
                        if self.flags.sets {
                            class_nest_level += 1;
                        }
                    } else if c == u32::from(b']') {
                        if class_nest_level == 0 {
                            break;
                        }
                        class_nest_level -= 1;
                    }
                }
            } else if n == u32::from(b'(') {
                if self.current == u32::from(b'?') {
                    self.advance();
                    if self.current != u32::from(b'<') {
                        continue;
                    }
                    self.advance();
                    if self.current == u32::from(b'=') || self.current == u32::from(b'!') {
                        continue;
                    }
                    self.has_named_captures = true;
                }
                capture_count += 1;
            }
        }
        self.capture_count = capture_count;
        self.is_scanned_for_captures = true;
        self.reset(saved_position);
    }

    /// `ParseBackReferenceIndex`: the group number when `\N...` is a backreference.
    fn back_reference_index(&mut self) -> Option<usize> {
        let start = self.position();
        let mut value = (self.next() - u32::from(b'0')) as usize;
        self.advance_by(2);
        loop {
            let c = self.current;
            if is_decimal_digit(c) {
                value = 10 * value + (c - u32::from(b'0')) as usize;
                if value > MAX_CAPTURES {
                    self.reset(start);
                    return None;
                }
                self.advance();
            } else {
                break;
            }
        }
        if value > self.captures_started {
            if !self.is_scanned_for_captures {
                self.scan_for_captures(InClass::No);
            }
            if value > self.capture_count {
                self.reset(start);
                return None;
            }
        }
        Some(value)
    }

    /// `ParseCaptureGroupName`: a group name, read as unicode whatever the flags.
    fn capture_group_name(&mut self) -> Option<Vec<u16>> {
        self.rewind_by_one_codepoint();
        let mut name = Vec::new();
        let saved_force = self.force_unicode;
        self.force_unicode = true;
        let mut at_start = true;
        let result = loop {
            self.advance();
            let mut c = self.current;
            if c == u32::from(b'\\') && self.next() == u32::from(b'u') {
                self.advance_by(2);
                match self.unicode_escape() {
                    Some(value) => c = value,
                    None => {
                        self.report(Error::InvalidUnicodeEscape);
                        break None;
                    }
                }
                self.rewind_by_one_codepoint();
            }
            // The backslash is misclassified as both ID_Start and ID_Continue.
            if c == u32::from(b'\\') {
                self.report(Error::InvalidCaptureGroupName);
                break None;
            }
            if at_start {
                if !is_identifier_start(c) {
                    self.report(Error::InvalidCaptureGroupName);
                    break None;
                }
                push_code_unit(&mut name, c);
                at_start = false;
            } else if c == u32::from(b'>') {
                break Some(());
            } else if is_identifier_part(c) {
                push_code_unit(&mut name, c);
            } else {
                self.report(Error::InvalidCaptureGroupName);
                break None;
            }
        };
        self.force_unicode = saved_force;
        result?;
        self.advance();
        Some(name)
    }

    /// `ParseNamedBackReference`, at the `<` of `\k<name>`.
    fn named_back_reference(&mut self, state: &mut State, stack: &[State]) {
        if self.current != u32::from(b'<') {
            self.report(Error::InvalidNamedReference);
            return;
        }
        self.advance();
        let Some(name) = self.capture_group_name() else {
            return;
        };
        let inside = state.capture_name.as_ref() == Some(&name)
            || stack.iter().any(|s| s.capture_name.as_ref() == Some(&name));
        if inside {
            state.builder.add_empty();
        } else {
            self.named_back_references.push(name.clone());
            state.builder.add(Term::NamedBackRef(name));
        }
    }

    /// `PatchNamedBackReferences`: every name must be a capture's.
    fn patch_named_back_references(&self, tree: &mut Disjunction) -> Result<(), Error> {
        if self.named_back_references.is_empty() {
            return Ok(());
        }
        for name in &self.named_back_references {
            if !self.named_captures.iter().any(|(n, _)| n == name) {
                return Err(Error::InvalidNamedCaptureReference);
            }
        }
        // Replace each NamedBackRef by the group's number, iteratively.
        let names = &self.named_captures;
        let mut work: Vec<&mut Vec<Term>> = tree.iter_mut().collect();
        while let Some(terms) = work.pop() {
            for term in terms.iter_mut() {
                let mut term = term;
                // Unwrap quantifiers down to the atom.
                while let Term::Quantified { body, .. } = term {
                    term = body;
                }
                match term {
                    Term::NamedBackRef(name) => {
                        let index = names
                            .iter()
                            .find(|(n, _)| n == name)
                            .map(|(_, i)| *i)
                            .expect("checked above");
                        *term = Term::BackRef(index);
                    }
                    Term::Group(body) | Term::Capture(_, body) | Term::Lookaround { body, .. } => {
                        work.extend(body.iter_mut());
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// `HasNamedCaptures`.
    fn has_named_captures(&mut self, in_class: InClass) -> bool {
        if self.has_named_captures || self.is_scanned_for_captures {
            return self.has_named_captures;
        }
        self.scan_for_captures(in_class);
        self.has_named_captures
    }

    /// `ParseIntervalQuantifier`: `{n}`, `{n,}` or `{n,m}`, values capped at kInfinity.
    fn interval_quantifier(&mut self) -> Option<(u32, u32)> {
        let start = self.position();
        self.advance();
        if !is_decimal_digit(self.current) {
            self.reset(start);
            return None;
        }
        let read_number = |p: &mut Self| -> u32 {
            let mut value: u32 = 0;
            while is_decimal_digit(p.current) {
                let next = p.current - u32::from(b'0');
                if value > (INFINITY - next) / 10 {
                    while is_decimal_digit(p.current) {
                        p.advance();
                    }
                    return INFINITY;
                }
                value = 10 * value + next;
                p.advance();
            }
            value
        };
        let min = read_number(self);
        let max;
        if self.current == u32::from(b'}') {
            max = min;
            self.advance();
        } else if self.current == u32::from(b',') {
            self.advance();
            if self.current == u32::from(b'}') {
                max = INFINITY;
                self.advance();
            } else {
                max = read_number(self);
                if self.current != u32::from(b'}') {
                    self.reset(start);
                    return None;
                }
                self.advance();
            }
        } else {
            self.reset(start);
            return None;
        }
        Some((min, max))
    }

    /// `ParseOctalLiteral`: up to three octal digits, below 256.
    fn octal_literal(&mut self) -> u32 {
        let mut value = self.current.wrapping_sub(u32::from(b'0'));
        self.advance();
        if (u32::from(b'0')..=u32::from(b'7')).contains(&self.current) {
            value = value * 8 + self.current - u32::from(b'0');
            self.advance();
            if value < 32 && (u32::from(b'0')..=u32::from(b'7')).contains(&self.current) {
                value = value * 8 + self.current - u32::from(b'0');
                self.advance();
            }
        }
        value
    }

    /// `ParseHexEscape`.
    fn hex_escape(&mut self, length: usize) -> Option<u32> {
        let start = self.position();
        let mut value = 0;
        for _ in 0..length {
            let Some(d) = hex_value(self.current) else {
                self.reset(start);
                return None;
            };
            value = value * 16 + d;
            self.advance();
        }
        Some(value)
    }

    /// `ParseUnicodeEscape`, after `\u`.
    fn unicode_escape(&mut self) -> Option<u32> {
        if self.current == u32::from(b'{') && self.unicode_mode() {
            let start = self.position();
            self.advance();
            if let Some(value) = self.unlimited_length_hex_number(0x10FFFF)
                && self.current == u32::from(b'}')
            {
                self.advance();
                return Some(value);
            }
            self.reset(start);
            return None;
        }
        let result = self.hex_escape(4);
        if let Some(value) = result
            && self.unicode_mode()
            && is_lead(value)
            && self.current == u32::from(b'\\')
        {
            let start = self.position();
            if self.next() == u32::from(b'u') {
                self.advance_by(2);
                if let Some(trail) = self.hex_escape(4)
                    && is_trail(trail)
                {
                    return Some(0x10000 + ((value - 0xD800) << 10) + (trail - 0xDC00));
                }
            }
            self.reset(start);
        }
        result
    }

    /// `ParseUnlimitedLengthHexNumber`.
    fn unlimited_length_hex_number(&mut self, max_value: u32) -> Option<u32> {
        let mut x: u32 = 0;
        let mut d = hex_value(self.current)?;
        loop {
            x = x * 16 + d;
            if x > max_value {
                return None;
            }
            self.advance();
            match hex_value(self.current) {
                Some(next) => d = next,
                None => break,
            }
        }
        Some(x)
    }

    /// `ParseCharacterEscape`, at the backslash: the character, and whether it was a
    /// `\u` escape.
    fn character_escape(&mut self, in_class: InClass) -> (u32, bool) {
        self.advance();
        let c = self.current;
        match char::from_u32(c) {
            Some('f') => {
                self.advance();
                return (0x0C, false);
            }
            Some('n') => {
                self.advance();
                return (0x0A, false);
            }
            Some('r') => {
                self.advance();
                return (0x0D, false);
            }
            Some('t') => {
                self.advance();
                return (0x09, false);
            }
            Some('v') => {
                self.advance();
                return (0x0B, false);
            }
            Some('c') => {
                let control_letter = self.next();
                let letter = control_letter & !(u32::from(b'A') ^ u32::from(b'a'));
                if (u32::from(b'A')..=u32::from(b'Z')).contains(&letter) {
                    self.advance_by(2);
                    return (control_letter & 0x1F, false);
                }
                if self.unicode_mode() {
                    self.report(Error::InvalidUnicodeEscape);
                    return (0, false);
                }
                if in_class == InClass::Yes
                    && (is_decimal_digit(control_letter) || control_letter == u32::from(b'_'))
                {
                    self.advance_by(2);
                    return (control_letter & 0x1F, false);
                }
                // The backslash as a literal character; `c` is read next.
                return (u32::from(b'\\'), false);
            }
            Some('0') if !is_decimal_digit(self.next()) => {
                self.advance();
                return (0, false);
            }
            Some('0'..='7') => {
                if self.unicode_mode() {
                    self.report(Error::InvalidClassEscape);
                    return (0, false);
                }
                return (self.octal_literal(), false);
            }
            Some('x') => {
                self.advance();
                if let Some(value) = self.hex_escape(2) {
                    return (value, false);
                }
                if self.unicode_mode() {
                    self.report(Error::InvalidEscape);
                    return (0, false);
                }
                return (u32::from(b'x'), false);
            }
            Some('u') => {
                self.advance();
                if let Some(value) = self.unicode_escape() {
                    return (value, true);
                }
                if self.unicode_mode() {
                    self.report(Error::InvalidUnicodeEscape);
                    return (0, false);
                }
                return (u32::from(b'u'), false);
            }
            _ => {}
        }
        if self.flags.sets && in_class == InClass::Yes && is_class_set_reserved_punctuator(c) {
            self.advance();
            return (c, false);
        }
        if self.unicode_mode() {
            if !is_syntax_character_or_slash(c) {
                self.report(Error::InvalidEscape);
                return (0, false);
            }
            self.advance();
            return (c, false);
        }
        if c == u32::from(b'c') {
            self.report(Error::InvalidEscape);
            return (0, false);
        }
        self.advance();
        // Advance before HasNamedCaptures, so that it does not scan from inside an escape.
        if c == u32::from(b'k') && self.has_named_captures(in_class) {
            self.report(Error::InvalidEscape);
            return (0, false);
        }
        (c, false)
    }

    /// `TryParseCharacterClassEscape` for `\p` and `\P`, at the backslash: `None` (and no
    /// error) where it is not one (without unicode mode).
    fn character_class_escape_property(
        &mut self,
        next: u32,
        in_class: InClass,
    ) -> Option<(bool, Property)> {
        if !self.unicode_mode() {
            return None;
        }
        let negated = next == u32::from(b'P');
        self.advance_by(2);
        let property = self
            .property_class_name()
            .and_then(|(name_1, name_2)| self.property(negated, &name_1, name_2.as_deref()));
        match property {
            Some(property) => Some((negated, property)),
            None => {
                self.report(if in_class == InClass::Yes {
                    Error::InvalidClassPropertyName
                } else {
                    Error::InvalidPropertyName
                });
                // V8 returns true (a class escape) with the error reported.
                Some((negated, Property::Lone(Lone::Any)))
            }
        }
    }

    /// `ParsePropertyClassName`: `{name}` or `{name=value}`.
    fn property_class_name(&mut self) -> Option<(String, Option<String>)> {
        if self.current != u32::from(b'{') {
            return None;
        }
        let mut name_1 = String::new();
        let mut name_2 = None;
        self.advance();
        while self.current != u32::from(b'}') && self.current != u32::from(b'=') {
            if !is_property_value_character(self.current) || !self.has_next() {
                return None;
            }
            name_1.push(char::from_u32(self.current).expect("ASCII"));
            self.advance();
        }
        if self.current == u32::from(b'=') {
            let mut value = String::new();
            self.advance();
            while self.current != u32::from(b'}') {
                if !is_property_value_character(self.current) || !self.has_next() {
                    return None;
                }
                value.push(char::from_u32(self.current).expect("ASCII"));
                self.advance();
            }
            name_2 = Some(value);
        }
        self.advance();
        Some((name_1, name_2))
    }

    /// `AddPropertyClassRange`: the property a name (and value) means, if V8 knows it.
    fn property(&self, negated: bool, name: &str, value: Option<&str>) -> Option<Property> {
        let lookup = |table: &'static [(&'static str, &'static str)], key: &str| {
            table
                .binary_search_by(|(k, _)| (*k).cmp(key))
                .ok()
                .map(|i| table[i].1)
        };
        match value {
            None => {
                let i = names::LONE.binary_search_by(|(k, _)| (*k).cmp(name)).ok()?;
                let lone = names::LONE[i].1;
                if let Lone::Strings(_) = lone {
                    // Properties of strings: only with v, never negated.
                    if !self.flags.sets || negated {
                        return None;
                    }
                }
                Some(Property::Lone(lone))
            }
            Some(value) => match name {
                "gc" | "General_Category" => lookup(&names::GC_VALUES, value).map(Property::Gc),
                "sc" | "Script" => lookup(&names::SCRIPT_VALUES, value).map(Property::Script),
                "scx" | "Script_Extensions" => {
                    lookup(&names::SCRIPT_VALUES, value).map(Property::ScriptExtensions)
                }
                _ => None,
            },
        }
    }

    /// `ParseCharacterClass`, at the `[`.
    fn character_class(&mut self) -> Option<Term> {
        self.advance();
        let mut negated = false;
        if self.current == u32::from(b'^') {
            negated = true;
            self.advance();
        }
        if self.current == u32::from(b']') {
            self.advance();
            return Some(if self.flags.sets {
                Term::SetClass(SetExpr {
                    negated,
                    op: SetOp::Union,
                    operands: Vec::new(),
                })
            } else {
                Term::Class(negated, Vec::new())
            });
        }
        if !self.flags.sets {
            let items = self.class_ranges();
            if self.failed() {
                return None;
            }
            if !self.has_more {
                self.report(Error::UnterminatedCharacterClass);
                return None;
            }
            self.advance();
            return Some(Term::Class(negated, items));
        }
        self.class_set_expression(negated).map(Term::SetClass)
    }

    /// `ParseClassRanges`.
    fn class_ranges(&mut self) -> Vec<ClassItem> {
        let mut items = Vec::new();
        while self.has_more && self.current != u32::from(b']') {
            let first = self.class_escape();
            if self.failed() {
                return items;
            }
            if self.current == u32::from(b'-') {
                self.advance();
                if !self.has_more {
                    // The caller reports the unterminated class.
                    if let ClassAtom::Char(c) = first {
                        items.push(ClassItem::Range(c, c));
                    } else if let ClassAtom::Item(item) = first {
                        items.push(item);
                    }
                    break;
                } else if self.current == u32::from(b']') {
                    first.add_to(&mut items);
                    items.push(ClassItem::Range(0x2D, 0x2D));
                    break;
                }
                let second = self.class_escape();
                if self.failed() {
                    return items;
                }
                match (&first, &second) {
                    (ClassAtom::Char(a), ClassAtom::Char(b)) => {
                        if a > b {
                            self.report(Error::OutOfOrderCharacterClass);
                            return items;
                        }
                        items.push(ClassItem::Range(*a, *b));
                    }
                    _ => {
                        // Either end is a class escape: the `-` is itself.
                        if self.unicode_mode() {
                            self.report(Error::InvalidCharacterClass);
                            return items;
                        }
                        first.add_to(&mut items);
                        items.push(ClassItem::Range(0x2D, 0x2D));
                        second.add_to(&mut items);
                    }
                }
            } else {
                first.add_to(&mut items);
            }
        }
        items
    }

    /// `ParseClassEscape`: a class member that is one character or a class escape.
    fn class_escape(&mut self) -> ClassAtom {
        if self.current != u32::from(b'\\') {
            let c = self.current;
            self.advance();
            return ClassAtom::Char(c);
        }
        let next = self.next();
        match next {
            n if n == u32::from(b'b') => {
                self.advance_by(2);
                return ClassAtom::Char(0x08);
            }
            n if n == u32::from(b'-') && self.unicode_mode() => {
                self.advance_by(2);
                return ClassAtom::Char(n);
            }
            END => {
                self.report(Error::EscapeAtEndOfPattern);
                return ClassAtom::Char(0);
            }
            _ => {}
        }
        if next < 0x80 && b"dDsSwW".contains(&(next as u8)) {
            self.advance_by(2);
            return ClassAtom::Item(ClassItem::Escape(next as u8));
        }
        if (next == u32::from(b'p') || next == u32::from(b'P'))
            && let Some((negated, property)) =
                self.character_class_escape_property(next, InClass::Yes)
        {
            return ClassAtom::Item(ClassItem::Property(negated, property));
        }
        let (c, _) = self.character_escape(InClass::Yes);
        ClassAtom::Char(c)
    }

    /// `ParseCharacterClass` with `v`, after `[` and `^`.
    fn class_set_expression(&mut self, negated: bool) -> Option<SetExpr> {
        self.class_nesting += 1;
        if self.class_nesting > MAX_CLASS_NESTING {
            self.report(Error::StackOverflow);
            return None;
        }
        let result = self.class_set_expression_inner(negated);
        self.class_nesting -= 1;
        result
    }

    fn class_set_expression_inner(&mut self, negated: bool) -> Option<SetExpr> {
        let (operand, operand_type) = self.class_set_operand()?;
        if self.current == u32::from(b'-') && self.next() == u32::from(b'-') {
            return self.class_subtraction(negated, operand, operand_type);
        }
        if self.current == u32::from(b'&') && self.next() == u32::from(b'&') {
            return self.class_intersection(negated, operand, operand_type);
        }
        self.class_union(negated, operand, operand_type)
    }

    /// `ParseClassSetOperand`: an operand and its type.
    fn class_set_operand(&mut self) -> Option<(SetOperand, OperandType)> {
        let c = self.current;
        if c == u32::from(b'\\') {
            let next = self.next();
            if next == u32::from(b'q') {
                let strings = self.class_string_disjunction()?;
                return Some((
                    SetOperand::Strings(strings),
                    OperandType::ClassStringDisjunction,
                ));
            }
            if next < 0x80 && b"dDsSwW".contains(&(next as u8)) {
                self.advance_by(2);
                return Some((
                    SetOperand::Escape(next as u8),
                    OperandType::CharacterClassEscape,
                ));
            }
            if next == u32::from(b'p') || next == u32::from(b'P') {
                let (negated, property) =
                    self.character_class_escape_property(next, InClass::Yes)?;
                if self.failed() {
                    return None;
                }
                return Some((
                    SetOperand::Property(negated, property),
                    OperandType::CharacterClassEscape,
                ));
            }
        }
        if c == u32::from(b'[') {
            self.advance();
            let mut negated = false;
            if self.current == u32::from(b'^') {
                negated = true;
                self.advance();
            }
            if self.current == u32::from(b']') {
                self.advance();
                return Some((
                    SetOperand::Nested(SetExpr {
                        negated,
                        op: SetOp::Union,
                        operands: Vec::new(),
                    }),
                    OperandType::NestedClass,
                ));
            }
            let expr = self.class_set_expression(negated)?;
            return Some((SetOperand::Nested(expr), OperandType::NestedClass));
        }
        let c = self.class_set_character()?;
        Some((SetOperand::Range(c, c), OperandType::ClassSetCharacter))
    }

    /// `ParseClassSetCharacter`.
    fn class_set_character(&mut self) -> Option<u32> {
        let c = self.current;
        if c == u32::from(b'\\') {
            let next = self.next();
            if next == u32::from(b'b') {
                self.advance_by(2);
                return Some(0x08);
            }
            if next == END {
                self.report(Error::EscapeAtEndOfPattern);
                return None;
            }
            let (c, _) = self.character_escape(InClass::Yes);
            return if self.failed() { None } else { Some(c) };
        }
        if is_class_set_syntax_character(c) {
            self.report(Error::InvalidCharacterInClass);
            return None;
        }
        if is_double_punctuator_char(c) && self.next() == c {
            self.report(Error::InvalidClassSetOperation);
            return None;
        }
        self.advance();
        Some(c)
    }

    /// `ParseClassStringDisjunction`, at `\q`.
    fn class_string_disjunction(&mut self) -> Option<Vec<Vec<u32>>> {
        self.advance_by(2);
        if self.current != u32::from(b'{') {
            self.report(Error::InvalidEscape);
            return None;
        }
        self.advance();
        let mut strings = Vec::new();
        let mut string = Vec::new();
        while self.has_more && self.current != u32::from(b'}') {
            if self.current == u32::from(b'|') {
                strings.push(std::mem::take(&mut string));
                self.advance();
            } else {
                let c = self.class_set_character()?;
                string.push(c);
            }
        }
        strings.push(string);
        // A missing `}` is reported by ParseClassSetCharacter on what follows.
        self.advance();
        Some(strings)
    }

    /// `ParseClassUnion`.
    fn class_union(
        &mut self,
        negated: bool,
        first: SetOperand,
        first_type: OperandType,
    ) -> Option<SetExpr> {
        let mut operands = Vec::new();
        let mut may_contain_strings = may_contain_strings(&first, first_type);
        let mut last_type = first_type;
        // A ClassSetCharacter waits in `pending` until we know whether a range follows.
        let mut pending = match first {
            SetOperand::Range(c, _) if first_type == OperandType::ClassSetCharacter => Some(c),
            other => {
                operands.push(other);
                None
            }
        };
        while self.has_more && self.current != u32::from(b']') {
            if self.current == u32::from(b'-') {
                // A range and a subtraction do not mix.
                if self.next() == u32::from(b'-') {
                    self.report(Error::InvalidClassSetOperation);
                    return None;
                }
                self.advance();
                if !self.has_more {
                    break;
                }
                if last_type != OperandType::ClassSetCharacter {
                    self.report(Error::InvalidCharacterClass);
                    return None;
                }
                let from = pending.take().expect("a pending character");
                let (operand, operand_type) = self.class_set_operand()?;
                if operand_type != OperandType::ClassSetCharacter {
                    self.report(Error::InvalidCharacterClass);
                    return None;
                }
                let SetOperand::Range(to, _) = operand else {
                    unreachable!("a ClassSetCharacter is a one-character range");
                };
                if from > to {
                    self.report(Error::OutOfOrderCharacterClass);
                    return None;
                }
                operands.push(SetOperand::Range(from, to));
                last_type = OperandType::ClassSetRange;
            } else {
                if let Some(c) = pending.take() {
                    operands.push(SetOperand::Range(c, c));
                }
                let (operand, operand_type) = self.class_set_operand()?;
                last_type = operand_type;
                if operand_type == OperandType::ClassSetCharacter {
                    let SetOperand::Range(c, _) = operand else {
                        unreachable!("a ClassSetCharacter is a one-character range");
                    };
                    pending = Some(c);
                } else {
                    may_contain_strings |= may_contain_strings_of(&operand, operand_type);
                    operands.push(operand);
                }
            }
        }
        if !self.has_more {
            self.report(Error::UnterminatedCharacterClass);
            return None;
        }
        if let Some(c) = pending.take() {
            operands.push(SetOperand::Range(c, c));
        }
        self.advance();
        if negated && may_contain_strings {
            self.report(Error::NegatedCharacterClassWithStrings);
            return None;
        }
        Some(SetExpr {
            negated,
            op: SetOp::Union,
            operands,
        })
    }

    /// `ParseClassIntersection`, at `&&`.
    fn class_intersection(
        &mut self,
        negated: bool,
        first: SetOperand,
        first_type: OperandType,
    ) -> Option<SetExpr> {
        let mut may_contain_strings = may_contain_strings(&first, first_type);
        let mut operands = vec![first];
        while self.has_more && self.current != u32::from(b']') {
            if self.current != u32::from(b'&') || self.next() != u32::from(b'&') {
                self.report(Error::InvalidClassSetOperation);
                return None;
            }
            self.advance_by(2);
            if self.current == u32::from(b'&') {
                self.report(Error::InvalidCharacterInClass);
                return None;
            }
            let (operand, operand_type) = self.class_set_operand()?;
            may_contain_strings &= may_contain_strings_of(&operand, operand_type);
            operands.push(operand);
        }
        if !self.has_more {
            self.report(Error::UnterminatedCharacterClass);
            return None;
        }
        if negated && may_contain_strings {
            self.report(Error::NegatedCharacterClassWithStrings);
            return None;
        }
        self.advance();
        Some(SetExpr {
            negated,
            op: SetOp::Intersection,
            operands,
        })
    }

    /// `ParseClassSubtraction`, at `--`.
    fn class_subtraction(
        &mut self,
        negated: bool,
        first: SetOperand,
        first_type: OperandType,
    ) -> Option<SetExpr> {
        if negated && may_contain_strings(&first, first_type) {
            self.report(Error::NegatedCharacterClassWithStrings);
            return None;
        }
        let mut operands = vec![first];
        while self.has_more && self.current != u32::from(b']') {
            if self.current != u32::from(b'-') || self.next() != u32::from(b'-') {
                self.report(Error::InvalidClassSetOperation);
                return None;
            }
            self.advance_by(2);
            let (operand, _) = self.class_set_operand()?;
            operands.push(operand);
        }
        if !self.has_more {
            self.report(Error::UnterminatedCharacterClass);
            return None;
        }
        self.advance();
        Some(SetExpr {
            negated,
            op: SetOp::Subtraction,
            operands,
        })
    }
}

/// A class member as `ParseClassEscape` gives it: one character, or a class escape.
enum ClassAtom {
    Char(u32),
    Item(ClassItem),
}

impl ClassAtom {
    fn add_to(&self, items: &mut Vec<ClassItem>) {
        match self {
            ClassAtom::Char(c) => items.push(ClassItem::Range(*c, *c)),
            ClassAtom::Item(item) => items.push(item.clone()),
        }
    }
}

/// `MayContainStrings` of an operand as `ParseClassSetOperand` gave it (the first operand of
/// a union, intersection or subtraction).
fn may_contain_strings(operand: &SetOperand, operand_type: OperandType) -> bool {
    may_contain_strings_of(operand, operand_type)
}

fn may_contain_strings_of(operand: &SetOperand, operand_type: OperandType) -> bool {
    match operand_type {
        OperandType::ClassSetCharacter | OperandType::ClassSetRange => false,
        OperandType::CharacterClassEscape | OperandType::ClassStringDisjunction => {
            match operand {
                // has_strings(): a string that is not one character (the empty one too).
                SetOperand::Strings(strings) => strings.iter().any(|s| s.len() != 1),
                SetOperand::Property(_, Property::Lone(Lone::Strings(_))) => true,
                _ => false,
            }
        }
        OperandType::NestedClass => match operand {
            SetOperand::Nested(expr) => set_may_contain_strings(expr),
            _ => false,
        },
    }
}

/// `RegExpClassSetExpression::may_contain_strings()`.
fn set_may_contain_strings(expr: &SetExpr) -> bool {
    let of = |operand: &SetOperand| match operand {
        SetOperand::Range(..) | SetOperand::Escape(_) => false,
        SetOperand::Property(_, Property::Lone(Lone::Strings(_))) => true,
        SetOperand::Property(..) => false,
        SetOperand::Strings(strings) => strings.iter().any(|s| s.len() != 1),
        SetOperand::Nested(expr) => set_may_contain_strings(expr),
    };
    match expr.op {
        SetOp::Union => expr.operands.iter().any(of),
        SetOp::Intersection => !expr.operands.is_empty() && expr.operands.iter().all(of),
        SetOp::Subtraction => expr.operands.first().is_some_and(of),
    }
}

/// `RegExpParserState::IsInsideCaptureGroup(index)`.
fn inside_capture_group(state: &State, stack: &[State], index: usize) -> bool {
    for s in std::iter::once(state).chain(stack.iter().rev()) {
        if s.group_type != GroupType::Capture {
            continue;
        }
        if index == s.capture_index {
            return true;
        }
        if index > s.capture_index {
            return false;
        }
    }
    false
}

/// `push_code_unit`.
fn push_code_unit(name: &mut Vec<u16>, c: u32) {
    if c <= 0xFFFF {
        name.push(c as u16);
    } else {
        let c = c - 0x10000;
        name.push(0xD800 + (c >> 10) as u16);
        name.push(0xDC00 + (c & 0x3FF) as u16);
    }
}

/// `IsIdentifierStart`: ID_Start, `$`, `_` and (misclassified, then refused) `\`.
fn is_identifier_start(c: u32) -> bool {
    matches!(c, 0x24 | 0x5C | 0x5F) || id_property(c, false)
}

/// `IsIdentifierPart`: ID_Continue, `$`, `_`, `\`, ZWNJ and ZWJ.
fn is_identifier_part(c: u32) -> bool {
    matches!(c, 0x24 | 0x5C | 0x5F | 0x200C | 0x200D) || id_property(c, true)
}

/// ID_Start or ID_Continue, as regress's Unicode tables have it (the same tables the
/// `\p{ID_Start}` V8 compares with are checked against node for every code point).
fn id_property(c: u32, continue_: bool) -> bool {
    use std::sync::OnceLock;
    static START: OnceLock<regress::Regex> = OnceLock::new();
    static CONTINUE: OnceLock<regress::Regex> = OnceLock::new();
    if c < 0x80 {
        let ascii = char::from_u32(c).expect("ASCII");
        return ascii.is_ascii_alphabetic() || (continue_ && ascii.is_ascii_digit());
    }
    if (0xD800..=0xDFFF).contains(&c) || c > 0x10FFFF {
        return false;
    }
    let cell = if continue_ { &CONTINUE } else { &START };
    let re = cell.get_or_init(|| {
        let pattern = if continue_ {
            "^\\p{ID_Continue}$"
        } else {
            "^\\p{ID_Start}$"
        };
        regress::Regex::with_flags(pattern, "u").expect("a valid property pattern")
    });
    let mut units = [0u16; 2];
    let text = char::from_u32(c)
        .expect("a scalar value")
        .encode_utf16(&mut units);
    re.find_from_utf16(text, 0).next().is_some()
}

/// Writes the parsed pattern out for regress (as code points for `Regex::from_unicode`), in
/// a canonical spelling regress reads as V8 read the original: every character but an ASCII
/// letter as an escape (`\u{...}` in unicode mode; `\uXXXX`, or the surrogate code unit
/// itself, which no escape could join to its neighbour, without), classes with their
/// members spelt out, property names in their short form, unnamed capture groups and
/// backreferences by number. regress's own reading of everything else (Annex B escapes,
/// group names, octal escapes, ...) never comes into play.
pub(super) fn emit(tree: &Disjunction, flags: Flags) -> Vec<u32> {
    let mut out = Vec::new();
    let unicode = flags.unicode || flags.sets;
    // An explicit stack of what is left to write.
    enum Item<'a> {
        Terms(&'a [Term]),
        Alternatives(&'a [Vec<Term>]),
        Text(&'static str),
        Quantifier(u32, u32, bool),
    }
    let mut work: Vec<Item> = vec![Item::Alternatives(tree)];
    while let Some(item) = work.pop() {
        match item {
            Item::Text(s) => out.extend(s.chars().map(u32::from)),
            Item::Quantifier(min, max, greedy) => {
                write_quantifier(&mut out, min, max);
                if !greedy {
                    out.push(u32::from('?'));
                }
            }
            Item::Alternatives(alternatives) => {
                for (k, alternative) in alternatives.iter().enumerate().rev() {
                    work.push(Item::Terms(alternative));
                    if k > 0 {
                        work.push(Item::Text("|"));
                    }
                }
            }
            Item::Terms(terms) => {
                let Some((term, rest)) = terms.split_first() else {
                    continue;
                };
                work.push(Item::Terms(rest));
                let mut term = term;
                let mut quantified = false;
                // A quantified term: the term, then its quantifier. A lookaround is put in a
                // group first (regress quantifies one only without `u`).
                while let Term::Quantified {
                    body,
                    min,
                    max,
                    greedy,
                } = term
                {
                    work.push(Item::Quantifier(*min, *max, *greedy));
                    if matches!(**body, Term::Lookaround { .. }) {
                        out.extend("(?:".chars().map(u32::from));
                        work.push(Item::Text(")"));
                    }
                    quantified = true;
                    term = body;
                }
                match term {
                    Term::Char(c) => write_char(&mut out, *c, unicode),
                    Term::Dot => out.push(u32::from('.')),
                    Term::Class(negated, items) => {
                        out.push(u32::from('['));
                        if *negated {
                            out.push(u32::from('^'));
                        }
                        for item in items {
                            match item {
                                ClassItem::Range(a, b) => {
                                    write_char(&mut out, *a, unicode);
                                    if b != a {
                                        out.push(u32::from('-'));
                                        write_char(&mut out, *b, unicode);
                                    }
                                }
                                ClassItem::Escape(e) => write_escape(&mut out, *e),
                                ClassItem::Property(negated, p) => {
                                    write_property(&mut out, *negated, p)
                                }
                            }
                        }
                        out.push(u32::from(']'));
                    }
                    Term::SetClass(expr) => write_set(&mut out, expr),
                    Term::Property(negated, p) => write_property(&mut out, *negated, p),
                    Term::Escape(e) => write_escape(&mut out, *e),
                    Term::Assertion(a) => out.extend(
                        match a {
                            Assertion::Start => "^",
                            Assertion::End => "$",
                            Assertion::Boundary => "\\b",
                            Assertion::NonBoundary => "\\B",
                        }
                        .chars()
                        .map(u32::from),
                    ),
                    Term::BackRef(n) => {
                        out.push(u32::from('\\'));
                        out.extend(n.to_string().chars().map(u32::from));
                    }
                    Term::NamedBackRef(_) => unreachable!("patched after the parse"),
                    // A group of one alternative that is not quantified is its terms (so
                    // that regress, which limits how deep groups nest, never sees them).
                    Term::Group(body) if !quantified && body.len() == 1 => {
                        work.push(Item::Terms(&body[0]));
                    }
                    Term::Group(body) => {
                        out.extend("(?:".chars().map(u32::from));
                        work.push(Item::Text(")"));
                        work.push(Item::Alternatives(body));
                    }
                    Term::Capture(_, body) => {
                        out.push(u32::from('('));
                        work.push(Item::Text(")"));
                        work.push(Item::Alternatives(body));
                    }
                    Term::Lookaround {
                        ahead,
                        positive,
                        body,
                    } => {
                        out.extend(
                            match (ahead, positive) {
                                (true, true) => "(?=",
                                (true, false) => "(?!",
                                (false, true) => "(?<=",
                                (false, false) => "(?<!",
                            }
                            .chars()
                            .map(u32::from),
                        );
                        work.push(Item::Text(")"));
                        work.push(Item::Alternatives(body));
                    }
                    Term::Quantified { .. } => unreachable!("unwrapped above"),
                }
            }
        }
    }
    out
}

fn write_quantifier(out: &mut Vec<u32>, min: u32, max: u32) {
    let text = match (min, max) {
        (0, INFINITY) => "*".to_string(),
        (1, INFINITY) => "+".to_string(),
        (0, 1) => "?".to_string(),
        (min, INFINITY) => format!("{{{min},}}"),
        (min, max) if min == max => format!("{{{min}}}"),
        (min, max) => format!("{{{min},{max}}}"),
    };
    out.extend(text.chars().map(u32::from));
}

/// One character: an ASCII letter as it is, anything else escaped.
fn write_char(out: &mut Vec<u32>, c: u32, unicode: bool) {
    if c < 0x80 && char::from_u32(c).is_some_and(|c| c.is_ascii_alphabetic()) {
        out.push(c);
    } else if unicode {
        out.extend(format!("\\u{{{c:X}}}").chars().map(u32::from));
    } else if (0xD800..=0xDFFF).contains(&c) {
        // regress joins `😀` into one code point even without `u`; a surrogate
        // given as itself is one code unit, whatever is next to it.
        out.push(c);
    } else {
        out.extend(format!("\\u{c:04X}").chars().map(u32::from));
    }
}

fn write_escape(out: &mut Vec<u32>, e: Escape) {
    out.push(u32::from('\\'));
    out.push(u32::from(e));
}

fn write_property(out: &mut Vec<u32>, negated: bool, p: &Property) {
    let name = match p {
        Property::Lone(Lone::Gc(v)) | Property::Gc(v) => format!("gc={v}"),
        Property::Lone(Lone::Any) => "Any".into(),
        Property::Lone(Lone::Ascii) => "ASCII".into(),
        Property::Lone(Lone::Assigned) => "Assigned".into(),
        Property::Lone(Lone::Binary(v)) | Property::Lone(Lone::Strings(v)) => (*v).into(),
        Property::Script(v) => format!("sc={v}"),
        Property::ScriptExtensions(v) => format!("scx={v}"),
    };
    let text = format!("\\{}{{{name}}}", if negated { 'P' } else { 'p' });
    out.extend(text.chars().map(u32::from));
}

/// A `v` class, its nested classes iteratively.
fn write_set(out: &mut Vec<u32>, expr: &SetExpr) {
    enum Item<'a> {
        Open(&'a SetExpr),
        Operand(&'a SetOperand),
        Text(&'static str),
    }
    let mut work = vec![Item::Open(expr)];
    while let Some(item) = work.pop() {
        match item {
            Item::Text(s) => out.extend(s.chars().map(u32::from)),
            Item::Open(expr) => {
                out.push(u32::from('['));
                if expr.negated {
                    out.push(u32::from('^'));
                }
                work.push(Item::Text("]"));
                let separator = match expr.op {
                    SetOp::Union => "",
                    SetOp::Intersection => "&&",
                    SetOp::Subtraction => "--",
                };
                for (k, operand) in expr.operands.iter().enumerate().rev() {
                    work.push(Item::Operand(operand));
                    if k > 0 && !separator.is_empty() {
                        work.push(Item::Text(separator));
                    }
                }
            }
            Item::Operand(operand) => match operand {
                SetOperand::Range(a, b) => {
                    write_char(out, *a, true);
                    if b != a {
                        out.push(u32::from('-'));
                        write_char(out, *b, true);
                    }
                }
                SetOperand::Escape(e) => write_escape(out, *e),
                SetOperand::Property(negated, p) => write_property(out, *negated, p),
                SetOperand::Strings(strings) => {
                    out.extend("\\q{".chars().map(u32::from));
                    for (k, s) in strings.iter().enumerate() {
                        if k > 0 {
                            out.push(u32::from('|'));
                        }
                        for &c in s {
                            write_char(out, c, true);
                        }
                    }
                    out.push(u32::from('}'));
                }
                SetOperand::Nested(expr) => work.push(Item::Open(expr)),
            },
        }
    }
}

/// `HasFewDifferentCharacters`: V8 prefers its regexp engine to a substring search for an
/// atom of at least three code units, each repeated about three times.
pub(super) fn has_few_different_characters(units: &[u16]) -> bool {
    let length = units.len().min(8);
    if length <= 2 {
        return false;
    }
    let mut found = [false; 128];
    let mut different = 0;
    for &u in &units[..length] {
        let ch = usize::from(u & 127);
        if !found[ch] {
            found[ch] = true;
            different += 1;
            if different * 3 > length {
                return false;
            }
        }
    }
    true
}
