//! [`JsString`]: a JavaScript string, UTF-16 code units exactly as a JS string holds them.
//!
//! A JS string is any sequence of 16-bit code units. Almost every one is well formed (each
//! surrogate in a pair) and is then text a Rust `String` holds. A JS string operation on valid
//! text can still make a *lone* surrogate: indexing a string (`$s[0]` of "😀" is `"\ud83d"`),
//! spreading one into an object (one code unit per key). A Rust `String` cannot hold that,
//! so a [`JsString`] keeps well-formed text as a `String` (the cheap path: no conversion, and
//! [`JsString::as_str`] borrows it) and any other string as its code units.
//!
//! Comparing, measuring, indexing and concatenating work on code units, as JS does: two
//! different halves are different strings, a half is not U+FFFD, and a lead half followed by
//! a trail half is the character they make ([`JsStringBuilder`]). A lone surrogate becomes
//! U+FFFD only where a string leaves as Rust text ([`JsString::to_string_lossy`], `Display`),
//! which is the bytes node writes for one when it encodes the string as UTF-8.

use std::borrow::Cow;
use std::fmt;

/// A JavaScript string (see [`crate::next::schema::value`]'s "Strings").
///
/// Built from text with `From<&str>`/`From<String>`, or from code units with
/// [`JsString::from_utf16`]; read as text with [`JsString::as_str`] (`None` when the string
/// holds a lone surrogate) or [`JsString::to_string_lossy`] (a lone surrogate as U+FFFD).
/// `==` compares code units.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct JsString(Repr);

/// Invariant: `Units` holds at least one lone surrogate, so that a string has one
/// representation and the derived `==` and `Hash` compare code units.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Repr {
    /// A well-formed string: its text.
    Text(String),
    /// A string with a lone surrogate: its code units.
    Units(Box<[u16]>),
}

impl JsString {
    /// The empty string.
    pub const fn new() -> JsString {
        JsString(Repr::Text(String::new()))
    }

    /// The string these code units are, whatever they are.
    pub fn from_utf16(units: &[u16]) -> JsString {
        match String::from_utf16(units) {
            Ok(text) => JsString(Repr::Text(text)),
            Err(_) => JsString(Repr::Units(units.into())),
        }
    }

    /// [`JsString::from_utf16`] of an owned buffer.
    pub fn from_utf16_vec(units: Vec<u16>) -> JsString {
        match String::from_utf16(&units) {
            Ok(text) => JsString(Repr::Text(text)),
            Err(_) => JsString(Repr::Units(units.into_boxed_slice())),
        }
    }

    /// `String.fromCharCode(unit)`: the one-code-unit string.
    pub fn from_code_unit(unit: u16) -> JsString {
        JsString::from_utf16(&[unit])
    }

    /// The text, when the string is well formed (holds no lone surrogate).
    pub fn as_str(&self) -> Option<&str> {
        match &self.0 {
            Repr::Text(text) => Some(text),
            Repr::Units(_) => None,
        }
    }

    /// `String.prototype.isWellFormed`: no lone surrogate.
    pub fn is_well_formed(&self) -> bool {
        matches!(self.0, Repr::Text(_))
    }

    /// The string as Rust text: borrowed when well formed, else with each lone surrogate
    /// replaced by U+FFFD (what node writes for one in UTF-8).
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        match &self.0 {
            Repr::Text(text) => Cow::Borrowed(text),
            Repr::Units(units) => Cow::Owned(String::from_utf16_lossy(units)),
        }
    }

    /// [`JsString::to_string_lossy`], owned.
    pub fn into_string_lossy(self) -> String {
        match self.0 {
            Repr::Text(text) => text,
            Repr::Units(units) => String::from_utf16_lossy(&units),
        }
    }

    /// The text, or the string itself back when it holds a lone surrogate.
    pub fn into_string(self) -> Result<String, JsString> {
        match self.0 {
            Repr::Text(text) => Ok(text),
            units => Err(JsString(units)),
        }
    }

    /// `s === ''`.
    pub fn is_empty(&self) -> bool {
        match &self.0 {
            Repr::Text(text) => text.is_empty(),
            Repr::Units(units) => units.is_empty(),
        }
    }

    /// `s.length`: the number of UTF-16 code units.
    pub fn len_utf16(&self) -> usize {
        match &self.0 {
            Repr::Text(text) => text.chars().map(char::len_utf16).sum(),
            Repr::Units(units) => units.len(),
        }
    }

    /// The code units, in order.
    pub fn code_units(&self) -> CodeUnits<'_> {
        CodeUnits(match &self.0 {
            Repr::Text(text) => CodeUnitsRepr::Text(text.encode_utf16()),
            Repr::Units(units) => CodeUnitsRepr::Units(units.iter()),
        })
    }

    /// The code units as a slice (borrowed when the string holds a lone surrogate).
    pub fn to_utf16(&self) -> Cow<'_, [u16]> {
        match &self.0 {
            Repr::Text(text) => Cow::Owned(text.encode_utf16().collect()),
            Repr::Units(units) => Cow::Borrowed(units),
        }
    }

    /// `this + other`: code units appended, so a lead half at the end of `self` and a
    /// trail half at the start of `other` become one character.
    pub fn concat(&self, other: &JsString) -> JsString {
        let mut out = JsStringBuilder::new();
        out.push_js(self);
        out.push_js(other);
        out.finish()
    }
}

impl Default for JsString {
    fn default() -> Self {
        JsString::new()
    }
}

/// The code units of a [`JsString`] ([`JsString::code_units`]).
#[derive(Clone)]
pub struct CodeUnits<'a>(CodeUnitsRepr<'a>);

#[derive(Clone)]
enum CodeUnitsRepr<'a> {
    Text(std::str::EncodeUtf16<'a>),
    Units(std::slice::Iter<'a, u16>),
}

impl Iterator for CodeUnits<'_> {
    type Item = u16;

    fn next(&mut self) -> Option<u16> {
        match &mut self.0 {
            CodeUnitsRepr::Text(units) => units.next(),
            CodeUnitsRepr::Units(units) => units.next().copied(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.0 {
            CodeUnitsRepr::Text(units) => units.size_hint(),
            CodeUnitsRepr::Units(units) => units.size_hint(),
        }
    }
}

impl From<&str> for JsString {
    fn from(text: &str) -> Self {
        JsString(Repr::Text(text.to_owned()))
    }
}

impl From<String> for JsString {
    fn from(text: String) -> Self {
        JsString(Repr::Text(text))
    }
}

impl From<&String> for JsString {
    fn from(text: &String) -> Self {
        JsString(Repr::Text(text.clone()))
    }
}

impl From<Cow<'_, str>> for JsString {
    fn from(text: Cow<'_, str>) -> Self {
        JsString(Repr::Text(text.into_owned()))
    }
}

impl From<char> for JsString {
    fn from(c: char) -> Self {
        JsString(Repr::Text(c.to_string()))
    }
}

impl PartialEq<str> for JsString {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == Some(other)
    }
}

impl PartialEq<&str> for JsString {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == Some(*other)
    }
}

impl PartialEq<String> for JsString {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == Some(other.as_str())
    }
}

impl PartialEq<JsString> for str {
    fn eq(&self, other: &JsString) -> bool {
        other == self
    }
}

impl PartialEq<JsString> for &str {
    fn eq(&self, other: &JsString) -> bool {
        other == self
    }
}

impl PartialEq<JsString> for String {
    fn eq(&self, other: &JsString) -> bool {
        other == self
    }
}

/// The text, a lone surrogate as U+FFFD.
impl fmt::Display for JsString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_lossy())
    }
}

/// As `str`'s `Debug`, a lone surrogate written `\u{d83d}`.
impl fmt::Debug for JsString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Repr::Text(text) => fmt::Debug::fmt(text.as_str(), f),
            Repr::Units(units) => {
                f.write_str("\"")?;
                for c in char::decode_utf16(units.iter().copied()) {
                    match c {
                        Ok(c) => {
                            for e in c.escape_debug() {
                                fmt::Write::write_char(f, e)?;
                            }
                        }
                        Err(e) => write!(f, "\\u{{{:x}}}", e.unpaired_surrogate())?,
                    }
                }
                f.write_str("\"")
            }
        }
    }
}

/// A JS string built by concatenation (`a + b`), code unit by code unit: text while every
/// piece is well formed, code units once one is not, so that two halves that meet become the
/// character they make. [`JsStringBuilder::finish`] gives the string;
/// [`JsStringBuilder::finish_lossy`] the text a renderer writes (a lone surrogate that is
/// still lone as U+FFFD).
#[derive(Default)]
pub struct JsStringBuilder(Builder);

#[derive(Default)]
enum Builder {
    #[default]
    Empty,
    Text(String),
    Units(Vec<u16>),
}

impl JsStringBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// The code units so far (switching to code units, which every later piece then joins).
    fn units(&mut self) -> &mut Vec<u16> {
        if !matches!(self.0, Builder::Units(_)) {
            let units = match std::mem::take(&mut self.0) {
                Builder::Text(text) => text.encode_utf16().collect(),
                _ => Vec::new(),
            };
            self.0 = Builder::Units(units);
        }
        match &mut self.0 {
            Builder::Units(units) => units,
            _ => unreachable!("just set"),
        }
    }

    /// Appends text.
    pub fn push_str(&mut self, text: &str) {
        match &mut self.0 {
            Builder::Empty => self.0 = Builder::Text(text.to_owned()),
            Builder::Text(out) => out.push_str(text),
            Builder::Units(units) => units.extend(text.encode_utf16()),
        }
    }

    /// Appends a character.
    pub fn push(&mut self, c: char) {
        match &mut self.0 {
            Builder::Empty => self.0 = Builder::Text(c.to_string()),
            Builder::Text(out) => out.push(c),
            Builder::Units(units) => units.extend(c.encode_utf16(&mut [0; 2]).iter()),
        }
    }

    /// Appends a JS string.
    pub fn push_js(&mut self, s: &JsString) {
        match &s.0 {
            Repr::Text(text) => self.push_str(text),
            Repr::Units(more) => self.units().extend_from_slice(more),
        }
    }

    /// Appends code units.
    pub fn push_units(&mut self, more: &[u16]) {
        match String::from_utf16(more) {
            // Well formed on its own, but a trail half at its start could still join a lead
            // half before it: only text after text stays text.
            Ok(text) if !matches!(self.0, Builder::Units(_)) => self.push_str(&text),
            _ => self.units().extend_from_slice(more),
        }
    }

    /// Nothing appended yet (or only empty strings).
    pub fn is_empty(&self) -> bool {
        match &self.0 {
            Builder::Empty => true,
            Builder::Text(text) => text.is_empty(),
            Builder::Units(units) => units.is_empty(),
        }
    }

    /// The string.
    pub fn finish(self) -> JsString {
        match self.0 {
            Builder::Empty => JsString::new(),
            Builder::Text(text) => JsString(Repr::Text(text)),
            Builder::Units(units) => JsString::from_utf16_vec(units),
        }
    }

    /// The string as Rust text (a lone surrogate as U+FFFD).
    pub fn finish_lossy(self) -> String {
        match self.0 {
            Builder::Empty => String::new(),
            Builder::Text(text) => text,
            Builder::Units(units) => String::from_utf16_lossy(&units),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEAD: u16 = 0xD83D;
    const TRAIL: u16 = 0xDE00;

    #[test]
    fn well_formed_text_is_text() {
        let s = JsString::from("a😀");
        assert_eq!(s.as_str(), Some("a😀"));
        assert!(s.is_well_formed());
        assert_eq!(s.len_utf16(), 3);
        assert_eq!(s.code_units().collect::<Vec<_>>(), [0x61, LEAD, TRAIL]);
        assert_eq!(JsString::from_utf16(&[0x61, LEAD, TRAIL]), s);
        assert_eq!(s, "a😀");
        assert!(JsString::new().is_empty() && JsString::default().is_empty());
    }

    #[test]
    fn a_lone_surrogate_is_kept() {
        let lead = JsString::from_code_unit(LEAD);
        let trail = JsString::from_code_unit(TRAIL);
        // node: '😀'[0] !== '😀'[1], '😀'[0] !== '\ufffd', '😀'[0].length === 1
        assert_ne!(lead, trail);
        assert_ne!(lead, JsString::from("\u{FFFD}"));
        assert_ne!(lead, "\u{FFFD}");
        assert_eq!(lead.len_utf16(), 1);
        assert_eq!(lead.as_str(), None);
        assert!(!lead.is_well_formed() && !lead.is_empty());
        assert_eq!(lead.to_string_lossy(), "\u{FFFD}");
        assert_eq!(lead.to_string(), "\u{FFFD}");
        assert_eq!(format!("{lead:?}"), "\"\\u{d83d}\"");
        assert_eq!(lead.clone().into_string(), Err(lead.clone()));
        assert_eq!(lead.to_utf16().as_ref(), [LEAD]);
    }

    #[test]
    fn halves_that_meet_are_the_character() {
        // node: '😀'[0] + '😀'[1] === '😀'
        let lead = JsString::from_code_unit(LEAD);
        let trail = JsString::from_code_unit(TRAIL);
        assert_eq!(lead.concat(&trail), JsString::from("😀"));
        assert_eq!(lead.concat(&trail).as_str(), Some("😀"));
        // ... and not the other way round: '😀'[1] + '😀'[0] has two lone surrogates.
        assert_eq!(trail.concat(&lead).to_string_lossy(), "\u{FFFD}\u{FFFD}");
        let mut b = JsStringBuilder::new();
        b.push_str("x");
        b.push_js(&lead);
        b.push_units(&[TRAIL, 0x79]);
        assert_eq!(b.finish(), JsString::from("x😀y"));
        let mut b = JsStringBuilder::new();
        b.push_js(&lead);
        b.push('z');
        assert_eq!(b.finish_lossy(), "\u{FFFD}z");
    }
}
