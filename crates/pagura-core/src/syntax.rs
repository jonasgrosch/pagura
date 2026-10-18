//! Bounded lexical and object parsing (ISO 32000-2 clauses 7.2 and 7.3).
//!
//! The parser never repairs input silently: every repair goes through a
//! [`Diagnostics`] sink, which rejects it in [`Recovery::Strict`] mode.

mod direct;

use direct::Context;

/// The direct-stream recovery state threaded through one parse, if enabled.
type Streams<'a, 'b, 'h> = Option<Context<'a, 'b, 'h>>;
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

/// The value of an ASCII hexadecimal digit (clauses 7.3.4.3 and 7.3.5),
/// either case.
pub const fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// What the parsed bytes are, so offsets become meaningful [`Position`]s.
///
/// New origins may be added along with new [`Position`] kinds, so matches
/// outside this crate need a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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

    /// Creates a strict parser at `offset`.
    ///
    /// # Errors
    ///
    /// Returns a syntax error if `offset` is beyond the end of `data`.
    pub fn with_offset(
        data: &'a [u8],
        offset: usize,
        origin: Origin,
        limits: Limits,
    ) -> Result<Self> {
        let mut parser = Self::new(data, origin, limits);
        parser.seek(offset)?;
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

    /// The unparsed input.
    fn rest(&self) -> &'a [u8] {
        self.data.get(self.position..).unwrap_or_default()
    }

    /// Counts one value against the per-object [`Limits::value_nodes`].
    fn count_node(&mut self) -> Result<()> {
        if self.nodes >= self.limits.value_nodes {
            return Err(self.limit("object value budget exceeded"));
        }
        self.nodes += 1;
        Ok(())
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
        let rest = self.rest();
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
        let mut context = Some(Context::new(handler, &self.limits));
        let value = self.value(0, &mut context)?;
        Ok((
            value,
            context.map_or_else(Vec::new, |context| context.found),
        ))
    }

    fn value(&mut self, depth: usize, streams: &mut Streams<'a, '_, '_>) -> Result<Value> {
        if depth > self.limits.depth {
            return Err(self.limit("object nesting depth exceeded"));
        }
        self.count_node()?;
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

    /// Parses a name at its solidus (clause 7.3.5).
    fn name(&mut self) -> Result<Name> {
        self.position += 1;
        self.name_body()
    }

    /// Parses the regular characters of a name after its solidus, decoding
    /// `#xx` escapes (clause 7.3.5). A name without escapes is copied once.
    fn name_body(&mut self) -> Result<Name> {
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
                if let Some(&[high, low]) = self
                    .data
                    .get(self.position..self.position.saturating_add(2))
                    && let (Some(high), Some(low)) = (hex_digit(high), hex_digit(low))
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

    fn array(&mut self, depth: usize, streams: &mut Streams<'a, '_, '_>) -> Result<Vec<Value>> {
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
        streams: &mut Streams<'a, '_, '_>,
    ) -> Result<Dictionary> {
        self.position += 2;
        self.duplicated_opener()?;
        let mut dictionary = Dictionary::new();
        let before_streams = streams.as_ref().map_or(0, |context| context.count);
        let mut repeated_key = false;
        loop {
            self.skip_whitespace();
            if self.dictionary_closer()? {
                if repeated_key
                    && streams
                        .as_ref()
                        .is_some_and(|context| context.count != before_streams)
                {
                    return Err(self.error(
                        "direct streams cannot be recovered across duplicate dictionary keys",
                    ));
                }
                return Ok(dictionary);
            }
            let key_position = self.position();
            let key = self.key(key_position)?;
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

    /// Skips a second `<<` before the first key, a repaired deviation.
    fn duplicated_opener(&mut self) -> Result<()> {
        self.skip_whitespace();
        if !self.rest().starts_with(b"<<") {
            return Ok(());
        }
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
        )
    }

    /// Consumes `>>`, or a stray `)` and then `>>` as a repaired deviation.
    /// Returns whether the dictionary ended.
    fn dictionary_closer(&mut self) -> Result<bool> {
        if self.rest().starts_with(b">>") {
            self.position += 2;
            return Ok(true);
        }
        if self.current() == Some(b')') {
            let saved = self.position;
            let at = self.position();
            self.position += 1;
            self.skip_whitespace();
            if self.rest().starts_with(b">>") {
                self.diagnostics.repair(
                    Diagnostic::new(
                        DICTIONARY_CLOSER,
                        "stray parenthesis before dictionary closer ignored",
                    )
                    .at(at),
                )?;
                self.position += 2;
                return Ok(true);
            }
            self.position = saved;
        }
        Ok(false)
    }

    /// Parses a dictionary key: a name, or a repaired bare keyword.
    fn key(&mut self, key_position: Position) -> Result<Name> {
        let bare = self.current() != Some(b'/');
        if bare {
            self.bare_key(key_position)?;
        }
        self.count_node()?;
        if bare { self.name_body() } else { self.name() }
    }

    /// Admits a key without its solidus (repair `syntax.key-solidus`) when
    /// the regular token starts with a letter and is not a reserved keyword.
    fn bare_key(&mut self, key_position: Position) -> Result<()> {
        let tail = self.rest();
        let length = tail.iter().take_while(|&&byte| is_regular(byte)).count();
        let token = tail.get(..length).unwrap_or_default();
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
        )
    }

    fn dictionary_value(
        &mut self,
        depth: usize,
        streams: &mut Streams<'a, '_, '_>,
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
                .data
                .get(..self.position)
                .and_then(|consumed| consumed.strip_suffix(b"endstream"))
                .map(<[u8]>::len)
                .ok_or_else(|| self.error("direct stream handler did not consume endstream"))?;
            if let Some(data) = data {
                if data.start > data.end || data.end > marker || data.start < start {
                    return Err(self.error("invalid direct stream source range"));
                }
                context
                    .record(start..self.offset(), data)
                    .map_err(|error| error.at(self.position()))?;
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
mod tests;
