//! The content tokenizer: operands, operators, compatibility sections and
//! inline images (7.8.2, 8.9.7, Annex A).
//!
//! Numbers, strings, names and arrays of numbers and strings are read in
//! place into buffers the reader keeps from one operation to the next, so
//! reading an operation allocates nothing; dictionaries, other arrays and
//! malformed tokens go through the object parser, whose values, errors and
//! repairs are the reference for all of them. Reading every operand with
//! the object parser instead, which looks ahead for a reference after each
//! integer and allocates strings and arrays, makes interpretation
//! measurably slower.

use crate::number::{self, Number};
use crate::op::{Buffers, Item, Operand, Slot};
use crate::token::{self, skip, word};
use crate::{Content, Op, Operator};
use pagura_core::repairs::{LEADING_ZERO_REFERENCE, ZERO_REFERENCE};
use pagura_core::syntax::{Origin, Parser, is_whitespace};
use pagura_core::{Diagnostic, Dictionary, Error, Limits, Meter, Position, Result, Value};

/// The most operands one operator takes, with room for long `SCN` lists:
/// DeviceN's 32 components and a pattern name.
const MAX_OPERANDS: usize = 32;

/// One operation and where its operator starts in the [`Content`]. Its
/// operands borrow from the [`Reader`] until the next operation is read.
#[derive(Debug, Clone, PartialEq)]
pub struct Operation<'r> {
    /// The operation.
    pub op: Op<'r>,
    /// The offset of the operator (of `BI` for an inline image).
    pub offset: usize,
}

/// Reads the operations of a [`Content`] in order.
///
/// Every token is charged to the caller's content budget before it is read.
/// Operands are bounded in number and may not be indirect references.
/// Unknown operators are skipped with their operands inside compatibility
/// sections (`BX` … `EX`) and are syntax errors elsewhere. Errors carry the
/// position of the token at fault.
#[derive(Debug)]
pub struct Reader<'c> {
    content: &'c Content<'c>,
    data: &'c [u8],
    /// The offset of the next token, or of white space before it.
    position: usize,
    /// The object parser, for the tokens not read in place.
    parser: Parser<'c>,
    limits: Limits,
    operands: Vec<Slot>,
    buffers: Buffers,
    /// Whether the operands belong to the operation last returned.
    taken: bool,
    compatibility: usize,
    /// The offsets of `BI` and of the data of an inline image whose header
    /// was yielded and whose data has not been read yet.
    inline: Option<(usize, usize)>,
}

impl<'c> Reader<'c> {
    /// A reader at the start of `content`. Operands are parsed under
    /// `limits`, and compatibility sections nest at most `limits.depth` deep.
    pub fn new(content: &'c Content<'c>, limits: &Limits) -> Self {
        Self {
            content,
            data: content.bytes(),
            position: 0,
            parser: Parser::new(content.bytes(), Origin::Buffer, *limits),
            limits: *limits,
            operands: Vec::new(),
            buffers: Buffers::default(),
            taken: false,
            compatibility: 0,
            inline: None,
        }
    }

    /// The content being read.
    pub fn content(&self) -> &'c Content<'c> {
        self.content
    }

    /// Whether no operands are pending and no compatibility section is open,
    /// as the end of a content sequence requires.
    pub fn is_balanced(&self) -> bool {
        (self.taken || self.operands.is_empty()) && self.compatibility == 0
    }

    /// Discards the operands no operator has taken, returning whether there
    /// were any: at the end of a content sequence they can only be dropped.
    pub fn discard_operands(&mut self) -> bool {
        if self.taken || self.operands.is_empty() {
            return false;
        }
        self.operands.clear();
        self.buffers.clear();
        true
    }

    /// The next operation, charging one unit of `tokens` per token, or
    /// `None` at the end of the content.
    ///
    /// # Errors
    ///
    /// Returns positioned `Syntax` errors for malformed tokens and operands,
    /// unknown operators outside compatibility sections and unbalanced `EX`;
    /// `Limit` errors when `tokens` or the operand or nesting budgets are
    /// exhausted; and `InvalidArgument` when the data of the previous inline
    /// image was not read.
    pub fn next(&mut self, tokens: &mut Meter) -> Result<Option<Operation<'_>>> {
        if self.inline.is_some() {
            return Err(Error::invalid_argument("inline image data was not read"));
        }
        if self.taken {
            self.operands.clear();
            self.buffers.clear();
            self.taken = false;
        }
        let content = self.content;
        let (operator, offset) = loop {
            self.position = skip(self.data, self.position);
            let offset = self.position;
            let Some(&byte) = self.data.get(offset) else {
                return Ok(None);
            };
            match self.token(byte, offset, tokens) {
                Ok(Some(operator)) => break (operator, offset),
                Ok(None) => {}
                Err(error) => return Err(content.locate(error, offset)),
            }
        };
        self.taken = true;
        if operator == Operator::BeginInlineImage {
            let dictionary = self
                .inline_header(offset, tokens)
                .map_err(|error| content.locate(error, offset))?;
            return Ok(Some(Operation {
                op: Op::InlineImage(Box::new(dictionary)),
                offset,
            }));
        }
        let op = Op::build(operator, &mut self.operands, &mut self.buffers)
            .and_then(|op| {
                compatibility(&mut self.compatibility, self.limits.depth, operator)?;
                Ok(op)
            })
            .map_err(|error| content.locate(error, offset))?;
        Ok(Some(Operation { op, offset }))
    }

    /// Reads the token starting with `byte` at `offset`: an operand is kept,
    /// an operator is returned.
    fn token(&mut self, byte: u8, offset: usize, tokens: &mut Meter) -> Result<Option<Operator>> {
        if tokens.charge(1).is_err() {
            return Err(self
                .completed_reference(offset)
                .unwrap_or(Error::limit("content token budget exceeded")));
        }
        // With no value budget, every operand is the object parser's error.
        let budget = self.limits.value_nodes > 0;
        let data = self.data;
        let next = data.get(offset + 1).copied();
        let operand = match byte {
            b'+' | b'-' | b'.' | b'0'..=b'9' if budget => {
                let (number, end) = number::read(data, offset)?;
                self.position = end;
                Operand::from(number)
            }
            b'(' | b'<' if budget && (byte, next) != (b'<', Some(b'<')) => {
                let start = self.buffers.bytes.len();
                if let Some(end) = self.string(byte, offset) {
                    self.position = end;
                    Operand::String(start..self.buffers.bytes.len())
                } else {
                    self.buffers.bytes.truncate(start);
                    self.parse(offset)?
                }
            }
            b'/' if budget => match token::name(data, offset, self.limits.string_bytes) {
                Some((name, end)) => {
                    self.position = end;
                    Operand::Name(pagura_core::Name::new(name))
                }
                None => self.parse(offset)?,
            },
            b'[' => match self.array(offset) {
                Some(operand) => operand,
                None => self.parse(offset)?,
            },
            b'/' | b'(' | b'<' | b'+' | b'-' | b'.' | b'0'..=b'9' => self.parse(offset)?,
            _ => {
                let word = word(data, offset);
                self.position = offset + word.len();
                match word {
                    b"" => return Err(Error::syntax("expected token").at(Position::Buffer(offset))),
                    b"true" => Operand::Value(Value::Boolean(true)),
                    b"false" => Operand::Value(Value::Boolean(false)),
                    b"null" => Operand::Value(Value::Null),
                    word => return self.operator(word, offset),
                }
            }
        };
        self.push(offset, operand)?;
        Ok(None)
    }

    /// Decodes the literal or hexadecimal string at `offset` into the byte
    /// buffer; `None` leaves it to the object parser.
    fn string(&mut self, byte: u8, offset: usize) -> Option<usize> {
        let limit = self.limits.string_bytes;
        let out = &mut self.buffers.bytes;
        if byte == b'(' {
            token::literal(self.data, offset, limit, out)
        } else {
            token::hex(self.data, offset, limit, out)
        }
    }

    /// Reads the array at `offset` in place when it holds only numbers and
    /// strings within the value budgets; `None` leaves it to the object
    /// parser, with the buffers as they were.
    fn array(&mut self, offset: usize) -> Option<Operand> {
        let (items, bytes) = (self.buffers.items.len(), self.buffers.bytes.len());
        let read = self.array_items(offset);
        if read.is_none() {
            self.buffers.items.truncate(items);
            self.buffers.bytes.truncate(bytes);
        }
        read.map(|end| {
            self.position = end;
            Operand::Array(items..self.buffers.items.len())
        })
    }

    fn array_items(&mut self, offset: usize) -> Option<usize> {
        // The array is a value at depth 0 and each element one at depth 1.
        if self.limits.value_nodes == 0 || self.limits.depth == 0 {
            return None;
        }
        let data = self.data;
        let mut nodes = 1;
        let mut position = offset + 1;
        loop {
            position = skip(data, position);
            let byte = *data.get(position)?;
            if byte == b']' {
                return Some(position + 1);
            }
            nodes += 1;
            if nodes > self.limits.value_nodes {
                return None;
            }
            let item = match byte {
                b'+' | b'-' | b'.' | b'0'..=b'9' => {
                    let (number, end) = number::read(data, position).ok()?;
                    position = end;
                    Item::Number(match number {
                        Number::Integer(n) => n as f64,
                        Number::Real(n) => n,
                    })
                }
                b'(' | b'<' if (byte, data.get(position + 1)) != (b'<', Some(&b'<')) => {
                    let start = self.buffers.bytes.len();
                    position = self.string(byte, position)?;
                    Item::Show(start..self.buffers.bytes.len())
                }
                _ => return None,
            };
            self.buffers.items.push(item);
        }
    }

    /// Reads the value at `offset` with the object parser.
    fn parse(&mut self, offset: usize) -> Result<Operand> {
        self.parser.seek(offset)?;
        let value = self.parser.parse()?;
        self.position = self.parser.offset();
        Ok(match value {
            Value::Integer(n) => Operand::Integer(n),
            Value::Real(n) => Operand::Real(n),
            Value::Name(name) => Operand::Name(name),
            Value::String(bytes) => {
                let start = self.buffers.bytes.len();
                self.buffers.bytes.extend_from_slice(&bytes);
                Operand::String(start..self.buffers.bytes.len())
            }
            value => Operand::Value(value),
        })
    }

    fn push(&mut self, offset: usize, operand: Operand) -> Result<()> {
        if self.operands.len() >= MAX_OPERANDS {
            return Err(self
                .completed_reference(offset)
                .or_else(|| self.started_reference(offset))
                .unwrap_or(Error::limit("content operand budget exceeded")));
        }
        if let Operand::Value(value) = &operand {
            no_references(value)?;
        }
        self.operands.push(Slot { offset, operand });
        Ok(())
    }

    fn operator(&mut self, word: &[u8], offset: usize) -> Result<Option<Operator>> {
        if word == b"R"
            && let Some(error) = self.completed_reference(offset)
        {
            return Err(error);
        }
        let Some(operator) = Operator::from_bytes(word) else {
            if self.compatibility > 0 {
                self.operands.clear();
                self.buffers.clear();
                return Ok(None);
            }
            return Err(Error::syntax("unknown content operator"));
        };
        Ok(Some(operator))
    }

    /// The error the object parser gives for an indirect reference `N G R`
    /// that the token at `offset` completes: the `R` after two operands, or
    /// the generation number after one, followed by `R`. The object parser
    /// reads the three tokens as one operand, charged as one token.
    fn completed_reference(&self, offset: usize) -> Option<Error> {
        let data = self.data;
        let token = word(data, offset);
        if token == b"R" {
            return match self.operands.as_slice() {
                [.., number, generation] => {
                    reference_error(data, number.offset, generation.offset, true)
                }
                _ => None,
            };
        }
        if word(data, skip(data, offset + token.len())) != b"R" {
            return None;
        }
        reference_error(data, self.operands.last()?.offset, offset, true)
    }

    /// The repair the object parser rejects while reading the reference
    /// `N G R` that the token at `offset` starts, before it counts the
    /// operand.
    fn started_reference(&self, offset: usize) -> Option<Error> {
        let data = self.data;
        let generation = skip(data, offset + word(data, offset).len());
        let after = skip(data, generation + word(data, generation).len());
        if word(data, after) != b"R" {
            return None;
        }
        reference_error(data, offset, generation, false)
    }

    /// Reads an inline image's dictionary through `ID` and the white-space
    /// byte (or CR LF) that ends it (8.9.7).
    fn inline_header(&mut self, offset: usize, tokens: &mut Meter) -> Result<Dictionary> {
        if !self.operands.is_empty() {
            return Err(Error::syntax("operands before BI"));
        }
        self.parser.seek(self.position)?;
        let mut dictionary = Dictionary::new();
        loop {
            tokens
                .charge(1)
                .map_err(|_| Error::limit("inline image dictionary budget exceeded"))?;
            if self.parser.peek() == Some(b'I') {
                if self.parser.word()? != b"ID" {
                    return Err(Error::syntax("inline image requires ID"));
                }
                break;
            }
            let key = self
                .parser
                .parse()?
                .as_name()
                .cloned()
                .ok_or(Error::syntax("inline image key must be name"))?;
            let value = self.parser.parse()?;
            no_references(&value)?;
            if dictionary.contains_key(key.as_bytes()) {
                return Err(Error::syntax("duplicate inline image key"));
            }
            dictionary.insert(key, value);
        }
        let end = self.parser.offset();
        let separator = *self
            .data
            .get(end)
            .filter(|&&b| is_whitespace(b))
            .ok_or(Error::syntax("ID requires whitespace"))?;
        let start = end
            + if separator == b'\r' && self.data.get(end + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
        self.position = start;
        self.inline = Some((offset, start));
        Ok(dictionary)
    }

    /// Reads the data of the inline image whose header [`Self::next`] just
    /// yielded, and the `EI` after it. `length` receives the bytes from the
    /// start of the data to the end of the content and returns the data's
    /// length, which depends on the image's filters and framing. An `EI`
    /// directly after the data, without white space, is read and marked in
    /// [`InlineData::unspaced`] for the caller to report or reject.
    ///
    /// # Errors
    ///
    /// Returns the errors of `length` and `Syntax` when `EI` does not follow
    /// the data, positioned as the inline image; `InvalidArgument` when no
    /// inline image header is pending.
    pub fn inline_image_data(
        &mut self,
        length: impl FnOnce(&[u8]) -> Result<usize>,
    ) -> Result<InlineData<'c>> {
        let (offset, start) = self
            .inline
            .take()
            .ok_or(Error::invalid_argument("no inline image header is pending"))?;
        self.image_data(start, length)
            .map_err(|error| self.content.locate(error, offset))
    }

    fn image_data(
        &mut self,
        start: usize,
        length: impl FnOnce(&[u8]) -> Result<usize>,
    ) -> Result<InlineData<'c>> {
        let data = self.data;
        let image = data
            .get(start..)
            .ok_or(Error::syntax("missing inline image data"))?;
        let length = length(image)?;
        let end = start
            .checked_add(length)
            .ok_or(Error::limit("inline offset overflow"))?;
        if end > data.len() {
            return Err(Error::syntax("offset outside input").at(Position::Buffer(end)));
        }
        let Some(&next) = data.get(end) else {
            return Err(Error::syntax("inline image requires EI"));
        };
        let spaced = is_whitespace(next);
        let at = if spaced { skip(data, end) } else { end };
        let word = word(data, at);
        if spaced && word.is_empty() {
            return Err(Error::syntax("expected token").at(Position::Buffer(at)));
        }
        if word != b"EI" {
            return Err(Error::syntax("inline image requires EI"));
        }
        self.position = at + word.len();
        let data = image
            .get(..length)
            .ok_or(Error::syntax("truncated inline data"))?;
        Ok(InlineData {
            data,
            unspaced: !spaced,
        })
    }
}

/// The data of an inline image, read by [`Reader::inline_image_data`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineData<'c> {
    /// The image data, as long as its header and filters determine.
    pub data: &'c [u8],
    /// Whether `EI` follows the data without the white space that separates
    /// tokens (7.2.3), which only a reported repair accepts.
    pub unspaced: bool,
}

/// Opens or closes a compatibility section for `BX` and `EX`.
fn compatibility(open: &mut usize, depth: usize, operator: Operator) -> Result<()> {
    match operator {
        Operator::BeginCompatibility => {
            *open = open
                .checked_add(1)
                .filter(|&n| n <= depth)
                .ok_or(Error::limit("content nesting budget exceeded"))?;
        }
        Operator::EndCompatibility => {
            *open = open.checked_sub(1).ok_or(Error::syntax("EX without BX"))?;
        }
        _ => {}
    }
    Ok(())
}

/// The object parser's error for the reference whose object and
/// generation numbers are the tokens at `number` and `generation`, when
/// both are unsigned integers it reads as one (7.3.10): content operands
/// are direct objects (7.8.2), and the object parser is strict, so its
/// reference repairs are errors too. Without `direct`, only the repairs.
fn reference_error(data: &[u8], number: usize, generation: usize, direct: bool) -> Option<Error> {
    let unsigned = |token: &[u8]| !token.is_empty() && token.iter().all(u8::is_ascii_digit);
    let (object, version) = (word(data, number), word(data, generation));
    if !unsigned(object) || !unsigned(version) {
        return None;
    }
    let value = std::str::from_utf8(object).ok()?.parse::<u32>().ok()?;
    std::str::from_utf8(version).ok()?.parse::<u16>().ok()?;
    let zeroes = |token: &[u8]| token.len() > 1 && token.first() == Some(&b'0');
    let position = Position::Buffer(number);
    let repair = if zeroes(object) || zeroes(version) {
        Diagnostic::new(
            LEADING_ZERO_REFERENCE,
            "leading zeros removed from reference object or generation number",
        )
    } else if value == 0 {
        Diagnostic::new(ZERO_REFERENCE, "object-zero reference interpreted as null")
    } else if direct {
        return Some(Error::syntax("indirect reference in content operand").at(position));
    } else {
        return None;
    };
    repair.at(position).to_error()
}

/// Content operands are direct objects (7.8.2).
fn no_references(value: &Value) -> Result<()> {
    match value {
        Value::Reference(_) => return Err(Error::syntax("indirect reference in content operand")),
        Value::Array(values) => {
            for value in values {
                no_references(value)?;
            }
        }
        Value::Dictionary(values) => {
            for (_, value) in values {
                no_references(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

impl From<Number> for Operand {
    fn from(number: Number) -> Self {
        match number {
            Number::Integer(n) => Self::Integer(n),
            Number::Real(n) => Self::Real(n),
        }
    }
}

#[cfg(test)]
mod tests;
