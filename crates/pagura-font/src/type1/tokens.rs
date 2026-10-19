//! Bounded lexical extraction of static Type 1 dictionary entries.
use crate::budget::Work;
use crate::read::bad;
use pagura_core::{Position, Result};
use pagura_raster::Transform;

/// Type 1 chapter 10: tokens and RD strings are at most 65,535 bytes.
const MAX_TOKEN_BYTES: usize = 65_535;

/// Tokens of one Type 1 buffer, charged to the parse's work budget.
pub(super) struct Tokens<'a, 'w, 'd> {
    data: &'a [u8],
    pub pos: usize,
    work: &'w mut Work<'d>,
}
impl<'a, 'w, 'd> Tokens<'a, 'w, 'd> {
    pub(super) const fn new(data: &'a [u8], work: &'w mut Work<'d>) -> Self {
        Self { data, pos: 0, work }
    }
    pub(super) fn work(&mut self) -> &mut Work<'d> {
        self.work
    }
    pub(super) fn next(&mut self) -> Result<Option<&'a [u8]>> {
        loop {
            let token = self.raw()?;
            if token.is_none_or(|t| !matches!(t.first(), Some(b'{' | b'('))) {
                return Ok(token);
            }
        }
    }
    pub(super) fn raw(&mut self) -> Result<Option<&'a [u8]>> {
        loop {
            while self.data.get(self.pos).is_some_and(|b| white(*b)) {
                self.pos += 1;
            }
            if self.data.get(self.pos) == Some(&b'%') {
                while self
                    .data
                    .get(self.pos)
                    .is_some_and(|b| !matches!(b, b'\r' | b'\n'))
                {
                    self.pos += 1;
                }
                continue;
            }
            let Some(&byte) = self.data.get(self.pos) else {
                return Ok(None);
            };
            self.work.work(1, Position::Buffer(self.pos))?;
            let start = self.pos;
            self.pos += 1;
            if matches!(byte, b'{' | b'(') {
                self.balanced(byte, if byte == b'{' { b'}' } else { b')' })?;
            } else if !b"[]<>}".contains(&byte) {
                while self
                    .data
                    .get(self.pos)
                    .is_some_and(|b| !white(*b) && !b"()[]{}<>/%".contains(b))
                {
                    self.pos += 1;
                }
            }
            if !matches!(byte, b'{' | b'(') {
                let len = self.pos - start;
                if len > MAX_TOKEN_BYTES {
                    return Err(bad(start, "Type 1 token exceeds 65,535 bytes"));
                }
                self.work.string(len, Position::Buffer(start))?;
            }
            return Ok(Some(&self.data[start..self.pos]));
        }
    }
    fn balanced(&mut self, open: u8, close: u8) -> Result<()> {
        let mut depth = 1_usize;
        while let Some(&byte) = self.data.get(self.pos) {
            self.pos += 1;
            if open == b'(' && byte == b'\\' {
                if self.pos < self.data.len() {
                    self.pos += 1;
                }
                continue;
            }
            if open == b'{' && byte == b'(' {
                self.balanced(b'(', b')')?;
                continue;
            }
            if open == b'{' && byte == b'%' {
                while self
                    .data
                    .get(self.pos)
                    .is_some_and(|b| !matches!(b, b'\r' | b'\n'))
                {
                    self.pos += 1;
                }
                continue;
            }
            if byte == open {
                depth += 1;
                self.work
                    .depth(depth, usize::MAX, Position::Buffer(self.pos))?;
            }
            if byte == close {
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
        }
        Err(bad(self.pos, "unterminated Type 1 string or procedure"))
    }
    pub(super) fn procedure(&mut self) -> Result<Option<&'a [u8]>> {
        let start = self.pos;
        let token = self.raw()?;
        if let Some(body) = token
            .and_then(|t| t.strip_prefix(b"{"))
            .and_then(|s| s.strip_suffix(b"}"))
        {
            Ok(Some(body))
        } else {
            self.pos = start;
            Ok(None)
        }
    }
    pub(super) fn peek(&mut self) -> Result<Option<&'a [u8]>> {
        let pos = self.pos;
        let token = self.next()?;
        self.pos = pos;
        Ok(token)
    }
    pub(super) fn required(&mut self) -> Result<&'a [u8]> {
        self.next()?
            .ok_or_else(|| bad(self.pos, "truncated Type 1 dictionary"))
    }
    pub(super) fn integer(&mut self) -> Result<usize> {
        let token = self.required()?;
        integer(token, self.pos)
    }
    pub(super) fn binary(&mut self, aliases: &super::aliases::Aliases) -> Result<&'a [u8]> {
        let size = self.integer()?;
        if size > MAX_TOKEN_BYTES {
            return Err(bad(self.pos, "Type 1 RD string exceeds 65,535 bytes"));
        }
        self.work.decoded(size, Position::Buffer(self.pos))?;
        if !aliases.reader(self.required()?) {
            return Err(super::unsupported(
                self.pos,
                "Type 1 binary reader is not implemented",
            ));
        }
        if !self.data.get(self.pos).is_some_and(|b| white(*b)) {
            return Err(bad(self.pos, "Type 1 RD needs a delimiter"));
        }
        self.pos += 1;
        let end = self
            .pos
            .checked_add(size)
            .ok_or_else(|| crate::read::limit(self.pos, "Type 1 binary size overflow"))?;
        let bytes = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| bad(self.pos, "truncated Type 1 binary data"))?;
        self.pos = end;
        Ok(bytes)
    }
    pub(super) fn matrix(&mut self) -> Result<Transform> {
        while self.data.get(self.pos).is_some_and(|b| white(*b)) {
            self.pos += 1;
        }
        let close = if self.data.get(self.pos) == Some(&b'{') {
            self.pos += 1;
            b"}".as_slice()
        } else if self.required()? == b"[" {
            b"]".as_slice()
        } else {
            return Err(super::unsupported(
                self.pos,
                "computed Type 1 FontMatrix is not implemented",
            ));
        };
        let mut numbers = [0.0; 6];
        for n in &mut numbers {
            *n = std::str::from_utf8(self.required()?)
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|n| n.is_finite())
                .ok_or_else(|| bad(self.pos, "invalid Type 1 matrix number"))?;
        }
        if self.required()? != close {
            return Err(bad(self.pos, "invalid Type 1 matrix size"));
        }
        let [a, b, c, d, e, f] = numbers;
        let matrix = Transform::new(a, b, c, d, e, f);
        if matrix.determinant() == 0.0 {
            return Err(bad(self.pos, "singular Type 1 FontMatrix"));
        }
        Ok(matrix)
    }
}
pub(super) fn integer(bytes: &[u8], at: usize) -> Result<usize> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| bad(at, "invalid Type 1 integer"))
}

fn white(byte: u8) -> bool {
    matches!(byte, 0 | 9 | 10 | 12 | 13 | 32)
}

#[cfg(test)]
mod tests {
    use super::Tokens;
    use crate::budget::{FontBudget, Work};
    use pagura_core::{Diagnostics, Limits, Recovery};

    #[test]
    fn balanced_literals_are_not_copied_or_mistaken_for_single_name_tokens() {
        let mut data = b"{ (ignored) % ignored brace }\n".to_vec();
        data.resize(70000, b' ');
        data.extend_from_slice(b"} end");
        let limits = Limits {
            string_bytes: 8,
            ..Limits::default()
        };
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(&limits), &mut diagnostics);
        let mut tokens = Tokens::new(&data, &mut work);
        assert_eq!(tokens.next().unwrap(), Some(b"end".as_slice()));
        assert_eq!(tokens.next().unwrap(), None);
    }
}
