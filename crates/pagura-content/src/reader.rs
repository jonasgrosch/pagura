//! The content tokenizer: operands, operators, compatibility sections and
//! inline images (7.8.2, 8.9.7, Annex A).

use crate::{Content, Op, Operator};
use pagura_core::syntax::{Origin, Parser, is_whitespace};
use pagura_core::{Dictionary, Error, Limits, Meter, Result, Value};

/// The most operands one operator takes, with room for long `SCN` lists:
/// DeviceN's 32 components and a pattern name.
const MAX_OPERANDS: usize = 32;

/// One operation and where its operator starts in the [`Content`].
#[derive(Debug, Clone, PartialEq)]
pub struct Operation {
    /// The operation.
    pub op: Op,
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
    parser: Parser<'c>,
    operands: Vec<Value>,
    compatibility: usize,
    depth: usize,
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
            parser: Parser::new(content.bytes(), Origin::Buffer, *limits),
            operands: Vec::new(),
            compatibility: 0,
            depth: limits.depth,
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
        self.operands.is_empty() && self.compatibility == 0
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
    pub fn next(&mut self, tokens: &mut Meter) -> Result<Option<Operation>> {
        if self.inline.is_some() {
            return Err(Error::invalid_argument("inline image data was not read"));
        }
        loop {
            let Some(byte) = self.parser.peek() else {
                return Ok(None);
            };
            let offset = self.parser.offset();
            match self.token(byte, offset, tokens) {
                Ok(Some(op)) => return Ok(Some(Operation { op, offset })),
                Ok(None) => {}
                Err(error) => return Err(self.content.locate(error, offset)),
            }
        }
    }

    /// Reads the token starting with `byte` at `offset`: an operand is kept,
    /// an operator becomes an operation.
    fn token(&mut self, byte: u8, offset: usize, tokens: &mut Meter) -> Result<Option<Op>> {
        tokens
            .charge(1)
            .map_err(|_| Error::limit("content token budget exceeded"))?;
        if matches!(
            byte,
            b'/' | b'(' | b'[' | b'<' | b'+' | b'-' | b'.' | b'0'..=b'9'
        ) {
            let value = self.parser.parse()?;
            self.operand(value)?;
            return Ok(None);
        }
        let word = self.parser.word()?;
        match word {
            b"true" => self.operand(Value::Boolean(true))?,
            b"false" => self.operand(Value::Boolean(false))?,
            b"null" => self.operand(Value::Null)?,
            b"BI" => return self.inline_header(offset, tokens).map(Some),
            _ => return self.operator(word),
        }
        Ok(None)
    }

    fn operand(&mut self, value: Value) -> Result<()> {
        if self.operands.len() >= MAX_OPERANDS {
            return Err(Error::limit("content operand budget exceeded"));
        }
        no_references(&value)?;
        self.operands.push(value);
        Ok(())
    }

    fn operator(&mut self, word: &[u8]) -> Result<Option<Op>> {
        let Some(operator) = Operator::from_bytes(word) else {
            if self.compatibility > 0 {
                self.operands.clear();
                return Ok(None);
            }
            return Err(Error::syntax("unknown content operator"));
        };
        let op = Op::parse(operator, std::mem::take(&mut self.operands))?;
        match op {
            Op::BeginCompatibility => {
                self.compatibility = self
                    .compatibility
                    .checked_add(1)
                    .filter(|&n| n <= self.depth)
                    .ok_or(Error::limit("content nesting budget exceeded"))?;
            }
            Op::EndCompatibility => {
                self.compatibility = self
                    .compatibility
                    .checked_sub(1)
                    .ok_or(Error::syntax("EX without BX"))?;
            }
            _ => {}
        }
        Ok(Some(op))
    }

    /// Reads an inline image's dictionary through `ID` and the white-space
    /// byte (or CR LF) that ends it (8.9.7).
    fn inline_header(&mut self, offset: usize, tokens: &mut Meter) -> Result<Op> {
        if !self.operands.is_empty() {
            return Err(Error::syntax("operands before BI"));
        }
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
        let data = self.parser.data();
        let separator = *data
            .get(end)
            .filter(|&&b| is_whitespace(b))
            .ok_or(Error::syntax("ID requires whitespace"))?;
        let start = end
            + if separator == b'\r' && data.get(end + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
        self.inline = Some((offset, start));
        Ok(Op::InlineImage(dictionary))
    }

    /// Reads the data of the inline image whose header [`Self::next`] just
    /// yielded, and the `EI` after it. `length` receives the bytes from the
    /// start of the data to the end of the content and returns the data's
    /// length, which depends on the image's filters and framing.
    ///
    /// # Errors
    ///
    /// Returns the errors of `length` and `Syntax` when `EI` does not follow
    /// the data, positioned as the inline image; `InvalidArgument` when no
    /// inline image header is pending.
    pub fn inline_image_data(
        &mut self,
        length: impl FnOnce(&[u8]) -> Result<usize>,
    ) -> Result<&'c [u8]> {
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
    ) -> Result<&'c [u8]> {
        let data = self
            .parser
            .data()
            .get(start..)
            .ok_or(Error::syntax("missing inline image data"))?;
        let length = length(data)?;
        let end = start
            .checked_add(length)
            .ok_or(Error::limit("inline offset overflow"))?;
        self.parser.seek(end)?;
        if self
            .parser
            .data()
            .get(end)
            .is_none_or(|&b| !is_whitespace(b))
            || self.parser.word()? != b"EI"
        {
            return Err(Error::syntax("inline image requires EI"));
        }
        data.get(..length)
            .ok_or(Error::syntax("truncated inline data"))
    }
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

#[cfg(test)]
mod tests;
