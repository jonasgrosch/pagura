//! Bounded lexical and object parsing (ISO 32000-2 clauses 7.2 and 7.3).
//!
//! The parser never repairs input silently: every repair goes through a
//! [`Diagnostics`] sink, which rejects it in [`Recovery::Strict`] mode.

use crate::{
    Diagnostic, Diagnostics, Dictionary, Error, Limits, Name, ObjectId, Position, Recovery, Repair,
    Result, Value,
};

/// Repair applied when a dictionary repeats a key (clause 7.3.7 forbids it):
/// the last value wins. Readers disagree here, so the repair is always reported.
pub const DUPLICATE_KEY: Repair = Repair::new("syntax.duplicate-key");

/// PDF white-space characters (clause 7.2.3, Table 1).
pub const fn is_whitespace(byte: u8) -> bool {
    matches!(byte, 0 | 9 | 10 | 12 | 13 | 32)
}

/// PDF delimiter characters (clause 7.2.3, Table 2).
pub const fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Regular characters: neither white space nor delimiters.
pub const fn is_regular(byte: u8) -> bool {
    !is_whitespace(byte) && !is_delimiter(byte)
}

const fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// What the parsed bytes are, so offsets become meaningful [`Position`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The complete original file.
    File,
    /// The decoded data of a stream object.
    Stream(ObjectId),
    /// A standalone buffer.
    Buffer,
}

impl Origin {
    /// The position of `offset` within input of this origin.
    pub const fn position(self, offset: usize) -> Position {
        match self {
            Self::File => Position::File(offset),
            Self::Stream(object) => Position::Stream { object, offset },
            Self::Buffer => Position::Buffer(offset),
        }
    }
}

/// A bounded, position-tracking PDF object parser over a byte slice.
#[derive(Debug)]
pub struct Parser<'a> {
    data: &'a [u8],
    position: usize,
    origin: Origin,
    limits: Limits,
    nodes: usize,
    diagnostics: Diagnostics,
}

impl<'a> Parser<'a> {
    /// Creates a strict parser at the start of `data`.
    pub fn new(data: &'a [u8], origin: Origin, limits: Limits) -> Self {
        Self {
            data,
            position: 0,
            origin,
            limits,
            nodes: 0,
            diagnostics: Diagnostics::new(Recovery::Strict),
        }
    }

    /// Creates a strict parser at `position`.
    ///
    /// # Errors
    ///
    /// Returns a syntax error if `position` is beyond the end of `data`.
    pub fn at(data: &'a [u8], position: usize, origin: Origin, limits: Limits) -> Result<Self> {
        let mut parser = Self::new(data, origin, limits);
        parser.seek(position)?;
        Ok(parser)
    }

    /// Selects the recovery mode for repairs applied during parsing.
    #[must_use]
    pub fn with_recovery(mut self, mode: Recovery) -> Self {
        self.diagnostics = Diagnostics::new(mode);
        self
    }

    /// The input being parsed.
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// The current offset into the input.
    pub fn offset(&self) -> usize {
        self.position
    }

    /// The current location as a [`Position`].
    pub fn position(&self) -> Position {
        self.origin.position(self.position)
    }

    /// Moves to `offset`.
    ///
    /// # Errors
    ///
    /// Returns a syntax error if `offset` is beyond the end of the input.
    pub fn seek(&mut self, offset: usize) -> Result<()> {
        if offset > self.data.len() {
            return Err(Error::syntax("offset outside input").at(self.origin.position(offset)));
        }
        self.position = offset;
        Ok(())
    }

    /// Repairs applied so far, moved out of the parser.
    pub fn take_diagnostics(&mut self) -> Vec<Diagnostic> {
        self.diagnostics.take()
    }

    fn error(&self, message: &'static str) -> Error {
        Error::syntax(message).at(self.position())
    }

    fn limit(&self, message: &'static str) -> Error {
        Error::limit(message).at(self.position())
    }

    fn current(&self) -> Option<u8> {
        self.data.get(self.position).copied()
    }

    fn next_byte(&mut self) -> Result<u8> {
        let byte = self
            .current()
            .ok_or_else(|| self.error("unexpected end of input"))?;
        self.position += 1;
        Ok(byte)
    }

    /// Skips white space and comments.
    pub fn skip_whitespace(&mut self) {
        loop {
            while self.current().is_some_and(is_whitespace) {
                self.position += 1;
            }
            if self.current() != Some(b'%') {
                return;
            }
            while self.current().is_some_and(|b| b != b'\r' && b != b'\n') {
                self.position += 1;
            }
        }
    }

    /// The next significant byte after white space and comments, without
    /// consuming it. `None` at end of input.
    pub fn peek(&mut self) -> Option<u8> {
        self.skip_whitespace();
        self.current()
    }

    /// Whether only white space and comments remain.
    pub fn at_end(&mut self) -> bool {
        self.peek().is_none()
    }

    /// Reads a run of regular characters, such as a keyword or operator.
    ///
    /// # Errors
    ///
    /// Returns a syntax error if the next token is not a regular run.
    pub fn word(&mut self) -> Result<&'a [u8]> {
        self.skip_whitespace();
        let start = self.position;
        while self.current().is_some_and(is_regular) {
            self.position += 1;
        }
        if start == self.position {
            return Err(self.error("expected token"));
        }
        Ok(&self.data[start..self.position])
    }

    /// Consumes `word` if it is the next complete token.
    pub fn keyword(&mut self, word: &[u8]) -> bool {
        self.skip_whitespace();
        let rest = &self.data[self.position..];
        let complete = rest.get(word.len()).is_none_or(|&b| !is_regular(b));
        if rest.starts_with(word) && complete {
            self.position += word.len();
            true
        } else {
            false
        }
    }

    /// Reads an unsigned decimal integer token.
    ///
    /// # Errors
    ///
    /// Returns a syntax error for anything but ASCII digits that fit `u64`.
    pub fn unsigned(&mut self) -> Result<u64> {
        let token = self.word()?;
        if !token.iter().all(u8::is_ascii_digit) {
            return Err(self.error("expected unsigned integer"));
        }
        std::str::from_utf8(token)
            .ok()
            .and_then(|text| text.parse().ok())
            .ok_or_else(|| self.error("integer overflow"))
    }

    /// Parses one complete object. The value-node budget applies per call.
    ///
    /// # Errors
    ///
    /// Returns syntax errors for malformed input and limit errors when a
    /// budget in [`Limits`] would be exceeded.
    pub fn parse(&mut self) -> Result<Value> {
        self.nodes = 0;
        self.value(0)
    }

    fn value(&mut self, depth: usize) -> Result<Value> {
        if depth > self.limits.depth {
            return Err(self.limit("object nesting depth exceeded"));
        }
        if self.nodes >= self.limits.value_nodes {
            return Err(self.limit("object value budget exceeded"));
        }
        self.nodes += 1;
        match self.peek() {
            Some(b'/') => self.name().map(Value::Name),
            Some(b'(') => self.literal_string().map(Value::String),
            Some(b'<') if self.data.get(self.position + 1) == Some(&b'<') => {
                self.dictionary(depth).map(Value::Dictionary)
            }
            Some(b'<') => self.hex_string().map(Value::String),
            Some(b'[') => self.array(depth).map(Value::Array),
            Some(b'+' | b'-' | b'.' | b'0'..=b'9') => self.number_or_reference(),
            Some(_) if self.keyword(b"true") => Ok(Value::Boolean(true)),
            Some(_) if self.keyword(b"false") => Ok(Value::Boolean(false)),
            Some(_) if self.keyword(b"null") => Ok(Value::Null),
            Some(_) => Err(self.error("unrecognized object")),
            None => Err(self.error("unexpected end of input")),
        }
    }

    fn push_bounded(&self, bytes: &mut Vec<u8>, byte: u8) -> Result<()> {
        if bytes.len() >= self.limits.string_bytes {
            return Err(self.limit("string or name length budget exceeded"));
        }
        bytes.push(byte);
        Ok(())
    }

    /// Parses a name at the current position (clause 7.3.5).
    fn name(&mut self) -> Result<Name> {
        self.position += 1;
        let mut bytes = Vec::new();
        while self.current().is_some_and(is_regular) {
            let mut byte = self.next_byte()?;
            if byte == b'#' {
                let high = hex_digit(self.next_byte()?);
                let low = hex_digit(self.next_byte()?);
                let (Some(high), Some(low)) = (high, low) else {
                    return Err(self.error("invalid name escape"));
                };
                byte = (high << 4) | low;
            }
            if byte == 0 {
                return Err(self.error("name contains a null byte"));
            }
            self.push_bounded(&mut bytes, byte)?;
        }
        Ok(Name::new(&bytes))
    }

    /// Parses a literal string (clause 7.3.4.2). Balanced parentheses nest
    /// without recursion, so nesting needs no separate budget.
    fn literal_string(&mut self) -> Result<Vec<u8>> {
        self.position += 1;
        let mut bytes = Vec::new();
        let mut open = 1_usize;
        loop {
            let byte = match self.next_byte()? {
                b'(' => {
                    open += 1;
                    b'('
                }
                b')' => {
                    open -= 1;
                    if open == 0 {
                        return Ok(bytes);
                    }
                    b')'
                }
                b'\\' => match self.escape()? {
                    Some(byte) => byte,
                    None => continue,
                },
                b'\r' => {
                    // An unescaped end-of-line marker of any kind reads as LF.
                    if self.current() == Some(b'\n') {
                        self.position += 1;
                    }
                    b'\n'
                }
                other => other,
            };
            self.push_bounded(&mut bytes, byte)?;
        }
    }

    /// Decodes the escape after a backslash; `None` for a line continuation.
    fn escape(&mut self) -> Result<Option<u8>> {
        let byte = match self.next_byte()? {
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'b' => 0x08,
            b'f' => 0x0c,
            b'\r' => {
                if self.current() == Some(b'\n') {
                    self.position += 1;
                }
                return Ok(None);
            }
            b'\n' => return Ok(None),
            first @ b'0'..=b'7' => {
                let mut code = u16::from(first - b'0');
                for _ in 0..2 {
                    match self.current() {
                        Some(digit @ b'0'..=b'7') => {
                            self.position += 1;
                            code = code * 8 + u16::from(digit - b'0');
                        }
                        _ => break,
                    }
                }
                // Clause 7.3.4.2: high-order overflow of \ddd is ignored.
                code.to_le_bytes()[0]
            }
            other => other,
        };
        Ok(Some(byte))
    }

    /// Parses a hexadecimal string (clause 7.3.4.3).
    fn hex_string(&mut self) -> Result<Vec<u8>> {
        self.position += 1;
        let mut bytes = Vec::new();
        let mut high = None;
        loop {
            let byte = self.next_byte()?;
            if byte == b'>' {
                break;
            }
            if is_whitespace(byte) {
                continue;
            }
            let digit = hex_digit(byte).ok_or_else(|| self.error("invalid hexadecimal string"))?;
            match high.take() {
                Some(high) => self.push_bounded(&mut bytes, (high << 4) | digit)?,
                None => high = Some(digit),
            }
        }
        // An odd final digit behaves as if followed by 0.
        if let Some(high) = high {
            self.push_bounded(&mut bytes, high << 4)?;
        }
        Ok(bytes)
    }

    fn array(&mut self, depth: usize) -> Result<Vec<Value>> {
        self.position += 1;
        let mut values = Vec::new();
        loop {
            if self.peek() == Some(b']') {
                self.position += 1;
                return Ok(values);
            }
            values.push(self.value(depth + 1)?);
        }
    }

    fn dictionary(&mut self, depth: usize) -> Result<Dictionary> {
        self.position += 2;
        let mut dictionary = Dictionary::new();
        loop {
            self.skip_whitespace();
            if self.data[self.position..].starts_with(b">>") {
                self.position += 2;
                return Ok(dictionary);
            }
            let key_position = self.position();
            if self.current() != Some(b'/') {
                return Err(self.error("dictionary key must be a name"));
            }
            if self.nodes >= self.limits.value_nodes {
                return Err(self.limit("object value budget exceeded"));
            }
            self.nodes += 1;
            let key = self.name()?;
            let value = self.value(depth + 1)?;
            if dictionary.insert(key, value).is_some() {
                self.diagnostics.repair(
                    Diagnostic::new(DUPLICATE_KEY, "dictionary repeats a key; last value kept")
                        .at(key_position),
                )?;
            }
        }
    }

    /// Parses a number, or `N G R` when the integer starts a reference.
    fn number_or_reference(&mut self) -> Result<Value> {
        let start = self.position;
        let token = self.word()?;
        let digits = token
            .strip_prefix(b"+")
            .or_else(|| token.strip_prefix(b"-"));
        let signed = digits.is_some();
        let digits = digits.unwrap_or(token);
        let dots = digits.iter().filter(|&&b| b == b'.').count();
        let valid = dots <= 1
            && digits.iter().any(u8::is_ascii_digit)
            && digits.iter().all(|&b| b.is_ascii_digit() || b == b'.');
        if !valid {
            return Err(Error::syntax("invalid number").at(self.origin.position(start)));
        }
        let text = std::str::from_utf8(token).map_err(|_| self.error("invalid number"))?;
        if dots == 1 {
            return match text.parse::<f64>() {
                Ok(number) if number.is_finite() => Ok(Value::Real(number)),
                _ => Err(self.error("real number out of range")),
            };
        }
        let integer: i64 = text
            .parse()
            .map_err(|_| self.error("integer out of range"))?;
        if !signed && let Some(reference) = self.reference_tail(integer) {
            return Ok(Value::Reference(reference));
        }
        Ok(Value::Integer(integer))
    }

    /// Tries to read `G R` after an object number; restores the position
    /// if the following tokens do not form a reference.
    fn reference_tail(&mut self, number: i64) -> Option<ObjectId> {
        let number = u32::try_from(number).ok().filter(|&n| n > 0)?;
        let saved = self.position;
        let generation = self
            .unsigned()
            .ok()
            .and_then(|g| u16::try_from(g).ok())
            .filter(|_| self.keyword(b"R"));
        if generation.is_none() {
            self.position = saved;
        }
        generation.map(|generation| ObjectId::new(number, generation))
    }
}

#[cfg(test)]
mod tests {
    use super::{Origin, Parser};
    use crate::{ErrorKind, Limits, Position, Recovery, Value};

    fn parse(data: &[u8]) -> crate::Result<Value> {
        Parser::new(data, Origin::Buffer, Limits::default()).parse()
    }

    #[test]
    fn value_budget_applies_per_object_not_per_parser() {
        let limits = Limits {
            value_nodes: 2,
            ..Limits::default()
        };
        let mut parser = Parser::new(b"1 2 3 4 5", Origin::Buffer, limits);
        for expected in 1..=5 {
            assert_eq!(parser.parse().unwrap(), Value::Integer(expected));
        }
    }

    #[test]
    fn deep_literal_string_nesting_is_valid() {
        let data = [b"(".repeat(200), b")".repeat(200)].concat();
        let expected = [b"(".repeat(199), b")".repeat(199)].concat();
        assert_eq!(parse(&data).unwrap(), Value::String(expected));
    }

    #[test]
    fn duplicate_keys_are_errors_or_reported_repairs() {
        let data = b"<< /A 1 /A 2 >>";
        let error = parse(data).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.position, Position::Buffer(8));
        let mut parser =
            Parser::new(data, Origin::Buffer, Limits::default()).with_recovery(Recovery::Repair);
        let Value::Dictionary(dictionary) = parser.parse().unwrap() else {
            panic!("expected dictionary");
        };
        assert_eq!(dictionary.get(b"A"), Some(&Value::Integer(2)));
        assert_eq!(parser.take_diagnostics().len(), 1);
    }

    #[test]
    fn stream_origin_reports_stream_positions() {
        let object = crate::ObjectId::new(4, 0);
        let error = Parser::new(b"  )", Origin::Stream(object), Limits::default())
            .parse()
            .unwrap_err();
        assert_eq!(error.position, Position::Stream { object, offset: 2 });
    }
}
