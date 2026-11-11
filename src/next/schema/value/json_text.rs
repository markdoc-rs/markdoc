//! `JSON.parse` of JSON text into a [`Value`], strings exact.
//!
//! `serde_json` cannot hold a lone surrogate (it rejects the escape `"\ud800"`), and a JS
//! string can: [`Value::from_json_text`] reads the text itself, so a `\udxxx` escape that is
//! not half of a pair becomes that code unit of a [`JsString`], as `JSON.parse` makes it.

use super::{JsString, Value};
use crate::types::JsObject;

/// One open array or object while reading (the reader is a loop, so any depth reads).
enum Open {
    Array(Vec<Value>),
    Object(JsObject<Value>, Option<String>),
}

struct Reader<'a> {
    text: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn error(&self, what: &str) -> String {
        format!("SyntaxError: {what} in JSON at position {}", self.pos)
    }

    fn skip_space(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.text.get(self.pos) {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.text.get(self.pos).copied()
    }

    fn expect(&mut self, b: u8) -> Result<(), String> {
        if self.peek() == Some(b) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error("Unexpected token"))
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, String> {
        if self.text[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(self.error("Unexpected token"))
        }
    }

    fn hex4(&mut self) -> Result<u16, String> {
        let digits = self
            .text
            .get(self.pos..self.pos + 4)
            .and_then(|d| std::str::from_utf8(d).ok())
            .filter(|d| d.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| self.error("Bad Unicode escape"))?;
        self.pos += 4;
        Ok(u16::from_str_radix(digits, 16).expect("four hex digits"))
    }

    /// A string literal (the opening quote is next), as JS holds it.
    fn string(&mut self) -> Result<JsString, String> {
        self.expect(b'"')?;
        let start = self.pos;
        // the common case: no escape, so the text itself (valid UTF-8) is the string
        while let Some(b) = self.peek() {
            match b {
                b'"' => {
                    let s = std::str::from_utf8(&self.text[start..self.pos]).expect("UTF-8");
                    self.pos += 1;
                    return Ok(JsString::from(s));
                }
                b'\\' => break,
                0..=0x1F => return Err(self.error("Bad control character in string literal")),
                _ => self.pos += 1,
            }
        }
        let mut units: Vec<u16> = std::str::from_utf8(&self.text[start..self.pos])
            .expect("UTF-8")
            .encode_utf16()
            .collect();
        loop {
            let Some(b) = self.peek() else {
                return Err(self.error("Unterminated string"));
            };
            match b {
                b'"' => {
                    self.pos += 1;
                    return Ok(JsString::from_utf16_vec(units));
                }
                b'\\' => {
                    self.pos += 1;
                    let Some(e) = self.peek() else {
                        return Err(self.error("Bad escaped character"));
                    };
                    self.pos += 1;
                    match e {
                        b'"' => units.push(0x22),
                        b'\\' => units.push(0x5C),
                        b'/' => units.push(0x2F),
                        b'b' => units.push(0x08),
                        b'f' => units.push(0x0C),
                        b'n' => units.push(0x0A),
                        b'r' => units.push(0x0D),
                        b't' => units.push(0x09),
                        b'u' => units.push(self.hex4()?),
                        _ => return Err(self.error("Bad escaped character")),
                    }
                }
                0..=0x1F => return Err(self.error("Bad control character in string literal")),
                _ => {
                    // one UTF-8 character
                    let len = match b {
                        0..=0x7F => 1,
                        0xC0..=0xDF => 2,
                        0xE0..=0xEF => 3,
                        _ => 4,
                    };
                    let c =
                        std::str::from_utf8(&self.text[self.pos..self.pos + len]).expect("UTF-8");
                    units.extend(c.encode_utf16());
                    self.pos += len;
                }
            }
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
            _ => return Err(self.error("No number after minus sign")),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !self.peek().is_some_and(|b| b.is_ascii_digit()) {
                return Err(self.error("Unterminated fractional number"));
            }
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.pos += 1;
            if let Some(b'+' | b'-') = self.peek() {
                self.pos += 1;
            }
            if !self.peek().is_some_and(|b| b.is_ascii_digit()) {
                return Err(self.error("Exponent part is missing a number"));
            }
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let text = std::str::from_utf8(&self.text[start..self.pos]).expect("ASCII");
        // Rust's parse rounds correctly, as JS's does
        Ok(Value::Number(text.parse::<f64>().expect("a JSON number")))
    }
}

impl Value {
    /// `JSON.parse(text)`: the value JSON text describes, with strings as JS holds them (a
    /// `\udxxx` escape that is not half of a pair is that lone code unit; see the module
    /// docs). Objects keep their keys in a JS object's order (array-index keys first,
    /// ascending; a repeated key keeps its first place and its last value), `__proto__` is an
    /// own key as `JSON.parse` makes it, and any depth reads (a loop). Object keys are Rust
    /// strings ([`JsObject`]), so a key's lone surrogate is U+FFFD. `Err` for text that is not
    /// JSON (a message saying where, not V8's).
    pub fn from_json_text(text: &str) -> Result<Value, String> {
        let mut r = Reader {
            text: text.as_bytes(),
            pos: 0,
        };
        let mut stack: Vec<Open> = Vec::new();
        loop {
            // a value starts here
            r.skip_space();
            let mut done = match r.peek() {
                Some(b'{') => {
                    r.pos += 1;
                    r.skip_space();
                    if r.peek() == Some(b'}') {
                        r.pos += 1;
                        Value::object(JsObject::new())
                    } else {
                        let key = r.string()?.into_string_lossy();
                        r.skip_space();
                        r.expect(b':')?;
                        stack.push(Open::Object(JsObject::new(), Some(key)));
                        continue;
                    }
                }
                Some(b'[') => {
                    r.pos += 1;
                    r.skip_space();
                    if r.peek() == Some(b']') {
                        r.pos += 1;
                        Value::array(Vec::new())
                    } else {
                        stack.push(Open::Array(Vec::new()));
                        continue;
                    }
                }
                Some(b'"') => Value::String(r.string()?),
                Some(b't') => r.literal("true", Value::Boolean(true))?,
                Some(b'f') => r.literal("false", Value::Boolean(false))?,
                Some(b'n') => r.literal("null", Value::Null)?,
                Some(b'-' | b'0'..=b'9') => r.number()?,
                Some(_) => return Err(r.error("Unexpected token")),
                None => return Err(r.error("Unexpected end of JSON input")),
            };
            // `done` is a whole value: put it into the open container, closing those it ends
            loop {
                match stack.last_mut() {
                    None => {
                        r.skip_space();
                        if r.pos != r.text.len() {
                            return Err(r.error("Unexpected non-whitespace character after JSON"));
                        }
                        return Ok(done);
                    }
                    Some(Open::Array(items)) => {
                        items.push(done);
                        r.skip_space();
                        match r.peek() {
                            Some(b',') => {
                                r.pos += 1;
                                break;
                            }
                            Some(b']') => {
                                r.pos += 1;
                                let Some(Open::Array(items)) = stack.pop() else {
                                    unreachable!("an array is open")
                                };
                                done = Value::array(items);
                            }
                            _ => return Err(r.error("Unexpected token")),
                        }
                    }
                    Some(Open::Object(object, key)) => {
                        object.insert(key.take().expect("a key"), done);
                        r.skip_space();
                        match r.peek() {
                            Some(b',') => {
                                r.pos += 1;
                                r.skip_space();
                                let next = r.string()?.into_string_lossy();
                                r.skip_space();
                                r.expect(b':')?;
                                *key = Some(next);
                                break;
                            }
                            Some(b'}') => {
                                r.pos += 1;
                                let Some(Open::Object(object, _)) = stack.pop() else {
                                    unreachable!("an object is open")
                                };
                                done = Value::object(object);
                            }
                            _ => return Err(r.error("Unexpected token")),
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::next::schema::value::json_stringify;

    fn units(v: &Value) -> Vec<u16> {
        v.as_js_str().expect("a string").to_utf16().into_owned()
    }

    #[test]
    fn lone_surrogates_are_read_exactly_and_written_back_as_json_stringify_does() {
        // node: JSON.parse('"\\ud83d"').charCodeAt(0) === 0xd83d; a pair is one character;
        // JSON.stringify writes a lone surrogate as the escape, a pair as the character
        let v =
            Value::from_json_text(r#"["\ud83d", "a\ude00b", "\ud83d\ude00", "\uD83Dx"]"#).unwrap();
        let Value::Array(items) = &v else {
            panic!("an array")
        };
        assert_eq!(units(items.get(0).unwrap()), [0xD83D]);
        assert_eq!(units(items.get(1).unwrap()), [0x61, 0xDE00, 0x62]);
        assert_eq!(units(items.get(2).unwrap()), [0xD83D, 0xDE00]);
        assert_eq!(units(items.get(3).unwrap()), [0xD83D, 0x78]);
        assert_eq!(
            json_stringify(&v, "").unwrap().unwrap(),
            r#"["\ud83d","a\ude00b","😀","\ud83dx"]"#
        );
    }

    #[test]
    fn json_parse_shapes() {
        // node: JSON.stringify(JSON.parse('{"b":1,"a":[true,null,-0,1e3,"x\\n\\u00e9"],"2":{},"b":2,"1":[]}'))
        //   === '{"1":[],"2":{},"b":2,"a":[true,null,0,1000,"x\\né"]}'
        let v = Value::from_json_text(
            r#" {"b":1,"a":[true,null,-0,1e3,"x\n\u00e9"],"2":{},"b":2,"1":[]} "#,
        )
        .unwrap();
        assert_eq!(
            json_stringify(&v, "").unwrap().unwrap(),
            r#"{"1":[],"2":{},"b":2,"a":[true,null,0,1000,"x\né"]}"#
        );
        for bad in [
            "",
            "[1,]",
            "{\"a\" 1}",
            "\"\u{1}\"",
            "01",
            "[1] x",
            "\"\\x\"",
            "-",
        ] {
            assert!(Value::from_json_text(bad).is_err(), "{bad:?}");
        }
        // any depth, as a loop
        let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        assert!(Value::from_json_text(&deep).is_ok());
    }
}
