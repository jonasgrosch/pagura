//! A small, complete JSON reader (RFC 8259) for fixture manifests and
//! generator outputs. Values are parsed eagerly into [`Json`]; object member
//! order is preserved.

use std::fmt;

/// Nesting deeper than this is rejected, so hostile or broken input cannot
/// exhaust the stack.
const MAX_DEPTH: usize = 128;

/// A JSON value. The variants are RFC 8259's closed set of value types.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number, as the nearest `f64`.
    Number(f64),
    /// A string, with escapes decoded.
    String(String),
    /// An array.
    Array(Vec<Json>),
    /// An object's members in source order.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// Parses one JSON text: a single value surrounded only by white space.
    ///
    /// # Errors
    ///
    /// A [`JsonError`] with the byte offset of the first violation.
    pub fn parse(text: &str) -> Result<Self, JsonError> {
        let mut reader = Reader {
            bytes: text.as_bytes(),
            text,
            at: 0,
        };
        let value = reader.value(0)?;
        reader.whitespace();
        if reader.at == reader.bytes.len() {
            Ok(value)
        } else {
            Err(reader.error("trailing data after the JSON value"))
        }
    }

    /// The member `key` of an object; the first one if it repeats.
    pub fn get(&self, key: &str) -> Option<&Self> {
        self.as_object()?
            .iter()
            .find_map(|(name, value)| (name == key).then_some(value))
    }

    /// Whether this is `null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// The boolean, if this is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The number, if this is one.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    /// The number as a `u64`, if it is a non-negative integer that `f64`
    /// represents exactly (at most 2^53).
    pub fn as_u64(&self) -> Option<u64> {
        let value = self.as_f64()?;
        let exact = value.fract() == 0.0 && (0.0..=9_007_199_254_740_992.0).contains(&value);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "only used when exact: a non-negative integer no larger than 2^53"
        )]
        let integer = value as u64;
        exact.then_some(integer)
    }

    /// The string, if this is one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    /// The elements, if this is an array.
    pub fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    /// The members in source order, if this is an object.
    pub fn as_object(&self) -> Option<&[(String, Self)]> {
        match self {
            Self::Object(members) => Some(members),
            _ => None,
        }
    }
}

/// Why a JSON text was rejected, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    offset: usize,
    message: &'static str,
}

impl JsonError {
    pub(crate) fn new(offset: usize, message: &'static str) -> Self {
        Self { offset, message }
    }

    /// Byte offset of the violation in the text.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// A fixed description of the violation.
    pub fn message(&self) -> &'static str {
        self.message
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.message, self.offset)
    }
}

impl std::error::Error for JsonError {}

struct Reader<'a> {
    bytes: &'a [u8],
    text: &'a str,
    at: usize,
}

impl Reader<'_> {
    fn error(&self, message: &'static str) -> JsonError {
        JsonError {
            offset: self.at,
            message,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn expect(&mut self, byte: u8, message: &'static str) -> Result<(), JsonError> {
        if self.peek() == Some(byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(self.error(message))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth > MAX_DEPTH {
            return Err(self.error("JSON nesting too deep"));
        }
        self.whitespace();
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Json::String),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(_) => Err(self.error("expected a JSON value")),
            None => Err(self.error("unexpected end of JSON")),
        }
    }

    fn literal(&mut self, word: &str, value: Json) -> Result<Json, JsonError> {
        if self
            .bytes
            .get(self.at..)
            .is_some_and(|rest| rest.starts_with(word.as_bytes()))
        {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.error("invalid JSON literal"))
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.at += 1;
        let mut members = Vec::new();
        self.whitespace();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Json::Object(members));
        }
        loop {
            self.whitespace();
            if self.peek() != Some(b'"') {
                return Err(self.error("expected an object key"));
            }
            let key = self.string()?;
            self.whitespace();
            self.expect(b':', "expected ':' after an object key")?;
            members.push((key, self.value(depth + 1)?));
            self.whitespace();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Json::Object(members));
                }
                _ => return Err(self.error("expected ',' or '}' in an object")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.at += 1;
        let mut values = Vec::new();
        self.whitespace();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Json::Array(values));
        }
        loop {
            values.push(self.value(depth + 1)?);
            self.whitespace();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Json::Array(values));
                }
                _ => return Err(self.error("expected ',' or ']' in an array")),
            }
        }
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.at += 1;
        let mut value = String::new();
        loop {
            let start = self.at;
            while self
                .peek()
                .is_some_and(|byte| byte != b'"' && byte != b'\\' && byte >= 0x20)
            {
                self.at += 1;
            }
            // The run ends at an ASCII byte, so it is a whole UTF-8 sequence.
            value.push_str(self.text.get(start..self.at).unwrap_or_default());
            match self.peek() {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(value);
                }
                Some(b'\\') => {
                    self.at += 1;
                    value.push(self.escape()?);
                }
                Some(_) => return Err(self.error("control character in a JSON string")),
                None => return Err(self.error("unterminated JSON string")),
            }
        }
    }

    fn escape(&mut self) -> Result<char, JsonError> {
        let character = match self.peek() {
            Some(b'"') => '"',
            Some(b'\\') => '\\',
            Some(b'/') => '/',
            Some(b'b') => '\u{8}',
            Some(b'f') => '\u{c}',
            Some(b'n') => '\n',
            Some(b'r') => '\r',
            Some(b't') => '\t',
            Some(b'u') => {
                self.at += 1;
                return self.unicode_escape();
            }
            Some(_) => return Err(self.error("invalid JSON escape")),
            None => return Err(self.error("unterminated JSON string")),
        };
        self.at += 1;
        Ok(character)
    }

    fn unicode_escape(&mut self) -> Result<char, JsonError> {
        const UNPAIRED: &str = "unpaired surrogate in a JSON string";
        let first = self.hex4()?;
        let code = if (0xd800..0xdc00).contains(&first) {
            if self.bytes.get(self.at..self.at + 2) != Some(b"\\u".as_slice()) {
                return Err(self.error(UNPAIRED));
            }
            self.at += 2;
            let low = self.hex4()?;
            if !(0xdc00..0xe000).contains(&low) {
                return Err(self.error(UNPAIRED));
            }
            0x10000 + ((first - 0xd800) << 10) + (low - 0xdc00)
        } else {
            first
        };
        char::from_u32(code).ok_or_else(|| self.error(UNPAIRED))
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let digits = self
            .text
            .get(self.at..self.at + 4)
            .filter(|digits| digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| self.error("invalid \\u escape in a JSON string"))?;
        self.at += 4;
        u32::from_str_radix(digits, 16)
            .map_err(|_| self.error("invalid \\u escape in a JSON string"))
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.at += 1;
        }
        self.at - start
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        match self.peek() {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.error("invalid JSON number")),
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            if self.digits() == 0 {
                return Err(self.error("invalid JSON number"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if self.digits() == 0 {
                return Err(self.error("invalid JSON number"));
            }
        }
        self.text
            .get(start..self.at)
            .and_then(|number| number.parse().ok())
            .filter(|number: &f64| number.is_finite())
            .map(Json::Number)
            .ok_or(JsonError {
                offset: start,
                message: "JSON number out of range",
            })
    }
}

#[cfg(test)]
mod tests {
    use super::Json;

    #[test]
    fn values_of_every_type_parse_with_member_order_kept() {
        let json = Json::parse(
            r#" {"b": [1, -2.5e2, 0.125, true, false, null], "a": {"x": "y"}, "e": [], "o": {}} "#,
        )
        .unwrap();
        let keys: Vec<_> = json
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(keys, ["b", "a", "e", "o"]);
        let b = json.get("b").unwrap().as_array().unwrap();
        assert_eq!(b[0].as_u64(), Some(1));
        assert_eq!(b[1].as_f64(), Some(-250.0));
        assert_eq!(b[1].as_u64(), None);
        assert_eq!(b[2].as_f64(), Some(0.125));
        assert_eq!((b[3].as_bool(), b[4].as_bool()), (Some(true), Some(false)));
        assert!(b[5].is_null());
        assert_eq!(json.get("a").unwrap().get("x").unwrap().as_str(), Some("y"));
        assert_eq!(json.get("e").unwrap().as_array(), Some([].as_slice()));
        assert_eq!(json.get("missing"), None);
        assert_eq!(
            Json::parse("9007199254740993").unwrap().as_u64(),
            Some(1 << 53)
        );
    }

    #[test]
    fn string_escapes_and_unicode_decode() {
        let json = Json::parse(r#""a\"b\\c\/d\b\f\n\r\t\u00e9\ud83d\ude00é""#).unwrap();
        assert_eq!(json.as_str(), Some("a\"b\\c/d\u{8}\u{c}\n\r\té😀é"));
    }

    #[test]
    fn malformed_texts_are_rejected_with_offsets() {
        for (text, offset) in [
            ("", 0),
            ("[1,]", 3),
            ("{\"a\" 1}", 5),
            ("{\"a\":1,}", 7),
            ("01", 1),
            ("1.", 2),
            ("1e", 2),
            ("-", 1),
            ("tru", 0),
            ("\"\\x\"", 2),
            ("\"\\ud800\"", 7),
            ("\"\\udc00\"", 7),
            ("\"a\nb\"", 2),
            ("\"open", 5),
            ("[1] 2", 4),
            ("1e999", 0),
            ("{1:2}", 1),
        ] {
            let error = Json::parse(text).unwrap_err();
            assert_eq!(error.offset(), offset, "{text:?}: {error}");
        }
        let deep = "[".repeat(200);
        assert_eq!(
            Json::parse(&deep).unwrap_err().message(),
            "JSON nesting too deep"
        );
    }
}
