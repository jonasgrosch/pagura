//! Bounded lexical and object parsing (ISO 32000-2 clauses 7.2 and 7.3).
//!
//! The parser never repairs input silently: every repair goes through a
//! [`Diagnostics`] sink, which rejects it in [`Recovery::Strict`] mode.

mod direct;

use direct::Context;
pub use direct::{DirectStream, DirectStreamHandler, PathComponent};

use crate::{
    Diagnostic, Diagnostics, Dictionary, Error, Limits, Name, ObjectId, Position, Recovery, Repair,
    Result, Value,
};

/// Repair applied when a dictionary repeats a key (clause 7.3.7 forbids it):
/// the last value wins. Readers disagree here, so the repair is always reported.
pub const DUPLICATE_KEY: Repair = Repair::new("syntax.duplicate-key");

/// A dictionary key with a missing solidus (7.3.7), accepted only when the
/// entire regular token begins with an ASCII letter. Reserved values are excluded.
pub const KEY_SOLIDUS: Repair = Repair::new("syntax.key-solidus");
/// A hash not followed by two hex digits is retained literally (7.3.5).
pub const NAME_ESCAPE: Repair = Repair::new("syntax.name-escape");
/// An undefined object-zero reference is read as null (7.3.10).
pub const ZERO_REFERENCE: Repair = Repair::new("syntax.reference-zero");
/// A lone closing parenthesis immediately before a dictionary closer (7.3.7).
pub const DICTIONARY_CLOSER: Repair = Repair::new("syntax.dictionary-closer");

/// A duplicated opening dictionary delimiter before its first key (7.3.7).
pub const DICTIONARY_OPENER: Repair = Repair::new("syntax.dictionary-opener");

/// A stream illegally used as a direct dictionary/array value (7.3.8.1).
pub const DIRECT_STREAM: Repair = Repair::new("syntax.direct-stream");

/// Leading zeros in reference identifiers (7.3.10, EC3 issue 379).
pub const LEADING_ZERO_REFERENCE: Repair = Repair::new("syntax.reference-leading-zero");

/// PDF white-space characters (clause 7.2.3, Table 1).
pub const fn is_whitespace(byte: u8) -> bool {
    matches!(byte, 0 | 9 | 10 | 12 | 13 | 32)
}

/// PDF delimiter characters (clause 7.2.3, Table 2). Braces are regular
/// outside Type 4 calculator functions; their tokenizer handles them separately.
pub const fn is_delimiter(byte: u8) -> bool {
    matches!(byte, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'/' | b'%')
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
        self.value(0, &mut None)
    }

    /// Parses a value while retaining source ranges of illegal direct streams.
    ///
    /// The callback starts before `stream` and must consume its complete payload
    /// and `endstream`, enforcing stream and dependency budgets. Return its data
    /// range to retain a descriptor, or `None` for a structural scan only. Neither
    /// callback nor parser may infer a missing terminator from end of input.
    /// Dictionaries and source bytes are unchanged; descriptors identify payloads
    /// separately. Descriptor metadata is capped by [`Limits::document_bytes`]
    /// before copying source paths. Normal [`Self::parse`] continues to reject direct streams.
    ///
    /// # Errors
    ///
    /// As [`Self::parse`], callback errors, or `syntax.direct-stream` in strict mode.
    pub fn parse_with_streams(
        &mut self,
        handler: &mut DirectStreamHandler<'a, '_>,
    ) -> Result<(Value, Vec<DirectStream>)> {
        self.nodes = 0;
        let mut context = Some(Context {
            handler,
            path: Vec::new(),
            found: Vec::new(),
            count: 0,
            bytes: 0,
        });
        let value = self.value(0, &mut context)?;
        Ok((
            value,
            context.map_or_else(Vec::new, |context| context.found),
        ))
    }

    fn value(&mut self, depth: usize, streams: &mut Option<Context<'a, '_>>) -> Result<Value> {
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
                self.dictionary_value(depth, streams)
            }
            Some(b'<') => self.hex_string().map(Value::String),
            Some(b'[') => self.array(depth, streams).map(Value::Array),
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
        let start = self.position;
        while self.current().is_some_and(|b| is_regular(b) && b != b'#') {
            self.position += 1;
            if self.position - start > self.limits.string_bytes {
                return Err(self.limit("string or name length budget exceeded"));
            }
        }
        if self.current() != Some(b'#') {
            return Ok(Name::new(&self.data[start..self.position]));
        }
        let mut bytes = self.data[start..self.position].to_vec();
        while self.current().is_some_and(is_regular) {
            let mut byte = self.next_byte()?;
            if byte == b'#' {
                let pair = self
                    .data
                    .get(self.position..self.position.saturating_add(2));
                if let Some((high, low)) =
                    pair.and_then(|pair| Some((hex_digit(pair[0])?, hex_digit(pair[1])?)))
                {
                    self.position += 2;
                    byte = (high << 4) | low;
                } else {
                    self.diagnostics.repair(
                        Diagnostic::new(
                            NAME_ESCAPE,
                            "invalid name escape retained as a literal hash",
                        )
                        .at(self.origin.position(self.position - 1)),
                    )?;
                }
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

    fn array(&mut self, depth: usize, streams: &mut Option<Context<'a, '_>>) -> Result<Vec<Value>> {
        self.position += 1;
        let mut values = Vec::new();
        loop {
            if self.peek() == Some(b']') {
                self.position += 1;
                return Ok(values);
            }
            if let Some(context) = streams {
                context.path.push(PathComponent::Index(values.len()));
            }
            values.push(self.value(depth + 1, streams)?);
            if let Some(context) = streams {
                context.path.pop();
            }
        }
    }

    fn dictionary(
        &mut self,
        depth: usize,
        streams: &mut Option<Context<'a, '_>>,
    ) -> Result<Dictionary> {
        self.position += 2;
        let mut dictionary = Dictionary::new();
        let before_streams = streams.as_ref().map_or(0, |context| context.count);
        let mut repeated_key = false;
        self.skip_whitespace();
        if self.data[self.position..].starts_with(b"<<") {
            let at = self.position();
            self.position += 2;
            self.skip_whitespace();
            if self.current() != Some(b'/') {
                return Err(self.error("dictionary key must be a name"));
            }
            self.diagnostics.repair(
                Diagnostic::new(
                    DICTIONARY_OPENER,
                    "duplicated dictionary opener before the first key ignored",
                )
                .at(at),
            )?;
        }

        loop {
            self.skip_whitespace();
            if self.data[self.position..].starts_with(b">>") {
                self.position += 2;
                return self.complete_dictionary(
                    dictionary,
                    streams.as_ref(),
                    before_streams,
                    repeated_key,
                );
            }
            let key_position = self.position();
            if self.current() == Some(b')') {
                let saved = self.position;
                self.position += 1;
                self.skip_whitespace();
                if self.data[self.position..].starts_with(b">>") {
                    self.diagnostics.repair(
                        Diagnostic::new(
                            DICTIONARY_CLOSER,
                            "stray parenthesis before dictionary closer ignored",
                        )
                        .at(key_position),
                    )?;
                    self.position += 2;
                    return self.complete_dictionary(
                        dictionary,
                        streams.as_ref(),
                        before_streams,
                        repeated_key,
                    );
                }
                self.position = saved;
            }
            let bare = self.current() != Some(b'/');
            if bare {
                self.bare_key(key_position)?;
            }
            if self.nodes >= self.limits.value_nodes {
                return Err(self.limit("object value budget exceeded"));
            }
            self.nodes += 1;
            let key = if bare {
                // Reuse name decoding, including escaped characters and bounds.
                // The dictionary opener guarantees a preceding byte exists.
                self.position -= 1;
                self.name()?
            } else {
                self.name()?
            };
            if let Some(context) = streams {
                context.path.push(PathComponent::Key(key.clone()));
            }
            let value = self.value(depth + 1, streams)?;
            if let Some(context) = streams {
                context.path.pop();
            }
            if dictionary.insert(key, value).is_some() {
                repeated_key = true;
                self.diagnostics.repair(
                    Diagnostic::new(DUPLICATE_KEY, "dictionary repeats a key; last value kept")
                        .at(key_position),
                )?;
            }
        }
    }

    fn bare_key(&mut self, key_position: Position) -> Result<()> {
        let tail = &self.data[self.position..];
        let length = tail.iter().take_while(|&&byte| is_regular(byte)).count();
        let token = &tail[..length];
        if !token.first().is_some_and(u8::is_ascii_alphabetic)
            || matches!(
                token,
                b"true" | b"false" | b"null" | b"stream" | b"endstream" | b"endobj" | b"obj" | b"R"
            )
        {
            return Err(self.error("dictionary key must be a name"));
        }
        self.diagnostics.repair(
            Diagnostic::new(KEY_SOLIDUS, "dictionary key has no leading solidus").at(key_position),
        )?;
        Ok(())
    }

    fn complete_dictionary(
        &self,
        dictionary: Dictionary,
        streams: Option<&Context<'a, '_>>,
        before: usize,
        repeated_key: bool,
    ) -> Result<Dictionary> {
        if repeated_key
            && streams
                .as_ref()
                .is_some_and(|context| context.count != before)
        {
            return Err(
                self.error("direct streams cannot be recovered across duplicate dictionary keys")
            );
        }
        Ok(dictionary)
    }

    fn dictionary_value(
        &mut self,
        depth: usize,
        streams: &mut Option<Context<'a, '_>>,
    ) -> Result<Value> {
        let start = self.offset();
        let dictionary = self.dictionary(depth, streams)?;
        let before = self.offset();
        if depth > 0
            && let Some(context) = streams
            && self.keyword(b"stream")
        {
            self.seek(before)?;
            self.diagnostics.repair(
                Diagnostic::new(
                    DIRECT_STREAM,
                    "stream used directly in a dictionary or array; source range retained",
                )
                .at(self.position()),
            )?;
            context.count += 1;
            let data = (context.handler)(self, &dictionary)?;
            let marker = self
                .offset()
                .checked_sub(9)
                .filter(|&marker| self.data.get(marker..self.offset()) == Some(b"endstream"))
                .ok_or_else(|| self.error("direct stream handler did not consume endstream"))?;
            if let Some(data) = data {
                if data.start > data.end || data.end > marker || data.start < start {
                    return Err(self.error("invalid direct stream source range"));
                }
                let capacity = if context.found.len() == context.found.capacity() {
                    context.found.capacity().saturating_mul(2).max(1)
                } else {
                    context.found.capacity()
                };
                let growth = capacity
                    .saturating_sub(context.found.capacity())
                    .saturating_mul(size_of::<DirectStream>());
                let bytes = context.path.iter().fold(growth, |sum, part| {
                    sum.saturating_add(part.retained_bytes())
                });
                context.bytes = context
                    .bytes
                    .checked_add(bytes)
                    .filter(|&bytes| bytes <= self.limits.document_bytes)
                    .ok_or_else(|| self.limit("direct stream metadata budget exceeded"))?;
                context
                    .found
                    .try_reserve_exact(capacity.saturating_sub(context.found.len()))
                    .map_err(|_| self.limit("direct stream metadata allocation failed"))?;
                context.found.push(DirectStream {
                    path: context.path.clone(),
                    source: start..self.offset(),
                    data,
                });
            }
        }
        Ok(Value::Dictionary(dictionary))
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
        let mut dots = 0;
        let valid = !digits.is_empty()
            && digits != b"."
            && digits.iter().all(|&byte| match byte {
                b'0'..=b'9' => true,
                b'.' => {
                    dots += 1;
                    dots == 1
                }
                _ => false,
            });
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
        if !signed && let Some((reference, generation_zeroes)) = self.reference_tail(integer) {
            if generation_zeroes || (token.len() > 1 && token.first() == Some(&b'0')) {
                self.diagnostics.repair(
                    Diagnostic::new(
                        LEADING_ZERO_REFERENCE,
                        "leading zeros removed from reference object or generation number",
                    )
                    .at(self.origin.position(start)),
                )?;
            }
            if reference.number == 0 {
                self.diagnostics.repair(
                    Diagnostic::new(ZERO_REFERENCE, "object-zero reference interpreted as null")
                        .at(self.origin.position(start)),
                )?;
                return Ok(Value::Null);
            }
            return Ok(Value::Reference(reference));
        }
        Ok(Value::Integer(integer))
    }

    /// Tries to read `G R` after an object number; restores the position
    /// if the following tokens do not form a reference.
    fn reference_tail(&mut self, number: i64) -> Option<(ObjectId, bool)> {
        let number = u32::try_from(number).ok()?;
        let saved = self.position;
        self.skip_whitespace();
        let start = self.position;
        // Most integer operands are not references. Establish the trailing R
        // before validating/converting the speculative generation token.
        let token = self
            .current()
            .filter(u8::is_ascii_digit)
            .and_then(|_| self.word().ok());
        if let Some(token) = token
            && self.keyword(b"R")
            && token.iter().all(u8::is_ascii_digit)
            && let Some(generation) = std::str::from_utf8(token)
                .ok()
                .and_then(|text| text.parse::<u16>().ok())
        {
            let zeroes = token.len() > 1 && self.data.get(start) == Some(&b'0');
            return Some((ObjectId::new(number, generation), zeroes));
        }
        self.position = saved;
        None
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
    fn name_prefixes_keep_decoded_limits_and_escape_positions() {
        let limits = Limits {
            string_bytes: 3,
            ..Limits::default()
        };
        for input in [b"/abc".as_slice(), b"/ab#63"] {
            let mut parser = Parser::new(input, Origin::Buffer, limits);
            assert_eq!(
                parser.parse().unwrap(),
                Value::Name(crate::Name::new(b"abc"))
            );
        }
        for input in [b"/abcd".as_slice(), b"/ab#63d"] {
            let mut parser = Parser::new(input, Origin::Buffer, limits);
            let error = parser.parse().unwrap_err();
            assert_eq!(error.kind, ErrorKind::Limit);
            assert_eq!(error.position, Position::Buffer(input.len()));
        }
        let mut parser = Parser::new(b"/ab#q", Origin::Buffer, limits);
        let error = parser.parse().unwrap_err();
        assert_eq!(error.repair, Some(super::NAME_ESCAPE));
        assert_eq!(error.position, Position::Buffer(3));
        let mut parser = Parser::new(b"/ab#00", Origin::Buffer, limits);
        let error = parser.parse().unwrap_err();
        assert_eq!(error.message, "name contains a null byte");
        assert_eq!(error.position, Position::Buffer(6));
    }

    #[test]
    fn reference_lookahead_restores_every_unsuccessful_candidate() {
        for input in [
            b"1 /A".as_slice(),
            b"1 +0 R",
            b"1 65536 R",
            b"1 2x R",
            b"1 0 Rigid",
            b"1 % comment\n2 3",
        ] {
            let mut parser = Parser::new(input, Origin::Buffer, Limits::default());
            assert_eq!(parser.parse().unwrap(), Value::Integer(1));
            assert_eq!(parser.offset(), 1);
        }
        let mut parser = Parser::new(b"1 % comment\n65535 R", Origin::Buffer, Limits::default());
        assert_eq!(
            parser.parse().unwrap(),
            Value::Reference(crate::ObjectId::new(1, 65535))
        );
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

#[cfg(test)]
mod recovery_tests {
    use super::{
        DICTIONARY_CLOSER, DICTIONARY_OPENER, KEY_SOLIDUS, NAME_ESCAPE, Origin, Parser,
        ZERO_REFERENCE,
    };
    use crate::{Limits, Recovery, Value};

    #[test]
    fn lexical_repairs_are_positioned_and_strict_errors_keep_their_tags() {
        for (bytes, tag) in [
            (b"<< Bare 1 >>".as_slice(), KEY_SOLIDUS),
            (b"/Name#Q", NAME_ESCAPE),
            (b"0 0 R", ZERO_REFERENCE),
            (b"<< /A 1 ) >>", DICTIONARY_CLOSER),
            (b"<< << /A 1 >>", DICTIONARY_OPENER),
        ] {
            let mut strict = Parser::new(bytes, Origin::File, Limits::default());
            assert_eq!(strict.parse().unwrap_err().repair, Some(tag));
            let mut repair =
                Parser::new(bytes, Origin::File, Limits::default()).with_recovery(Recovery::Repair);
            repair.parse().unwrap();
            assert_eq!(repair.take_diagnostics()[0].repair(), Some(tag));
        }
        let mut parser = Parser::new(b"/A# /B#2 /C#23 0 0 R", Origin::File, Limits::default())
            .with_recovery(Recovery::Repair);
        for expected in [b"A#".as_slice(), b"B#2", b"C#"] {
            assert_eq!(
                parser.parse().unwrap().as_name().unwrap().as_bytes(),
                expected
            );
        }
        assert_eq!(parser.parse().unwrap(), Value::Null);
        for bytes in [
            b"<< 3 4 >>".as_slice(),
            b"<< true 4 >>",
            b"<< stream 4 >>",
            b"<< /A 1 ) /B 2 >>",
        ] {
            assert!(
                Parser::new(bytes, Origin::File, Limits::default())
                    .with_recovery(Recovery::Repair)
                    .parse()
                    .is_err()
            );
        }
    }
}

#[cfg(test)]
mod identifier_tests {
    use super::{LEADING_ZERO_REFERENCE, Origin, Parser};
    use crate::{Limits, ObjectId, Position, Recovery, Value};

    #[test]
    fn identifier_zeroes_are_reported_only_after_a_complete_reference() {
        for bytes in [b"01 0 R".as_slice(), b"1 00 R", b"01 00 R"] {
            let error = Parser::new(bytes, Origin::File, Limits::default())
                .parse()
                .unwrap_err();
            assert_eq!(error.repair, Some(LEADING_ZERO_REFERENCE));
            assert_eq!(error.position, Position::File(0));
            let mut parser =
                Parser::new(bytes, Origin::File, Limits::default()).with_recovery(Recovery::Repair);
            assert_eq!(
                parser.parse().unwrap(),
                Value::Reference(ObjectId::new(1, 0))
            );
            assert_eq!(parser.take_diagnostics().len(), 1);
        }
        let mut parser = Parser::new(b"[01 00 1 0]", Origin::Buffer, Limits::default());
        assert_eq!(
            parser.parse().unwrap(),
            Value::Array(vec![
                Value::Integer(1),
                Value::Integer(0),
                Value::Integer(1),
                Value::Integer(0)
            ])
        );
        assert_eq!(parser.take_diagnostics(), []);
    }

    #[test]
    fn braces_are_regular_characters_in_ordinary_pdf_names() {
        let mut parser = Parser::new(b"[/A{B /C}D /E#7bF]", Origin::Buffer, Limits::default());
        assert_eq!(
            parser.parse().unwrap(),
            Value::Array(vec![
                Value::name(b"A{B"),
                Value::name(b"C}D"),
                Value::name(b"E{F")
            ])
        );
        assert_eq!(parser.take_diagnostics(), []);
        assert!(!super::is_delimiter(b'{'));
        assert!(!super::is_delimiter(b'}'));
    }
}
