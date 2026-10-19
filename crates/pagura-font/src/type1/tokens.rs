//! Bounded lexical extraction of static Type 1 dictionary entries.
use crate::read::{bad, cap};
use pagura_core::{Limits, Result};
use pagura_raster::Transform;

pub(super) struct Tokens<'a> {
    data: &'a [u8],
    pub pos: usize,
    limits: &'a Limits,
    operations: usize,
}
impl<'a> Tokens<'a> {
    pub(super) fn new(data: &'a [u8], limits: &'a Limits) -> Self {
        Self {
            data,
            pos: 0,
            limits,
            operations: 0,
        }
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
            self.operations = self.operations.saturating_add(1);
            cap(self.operations, self.limits.operations, self.pos)?;
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
                cap(self.pos - start, self.limits.string_bytes.min(65535), start)?;
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
                cap(depth, self.limits.depth.min(64), self.pos)?;
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
        cap(size, self.limits.decoded_bytes.min(65535), self.pos)?;
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
    use pagura_core::Limits;

    #[test]
    fn balanced_literals_are_not_copied_or_mistaken_for_single_name_tokens() {
        let mut data = b"{ (ignored) % ignored brace }\n".to_vec();
        data.resize(70000, b' ');
        data.extend_from_slice(b"} end");
        let limits = Limits {
            string_bytes: 8,
            ..Limits::default()
        };
        let mut tokens = Tokens::new(&data, &limits);
        assert_eq!(tokens.next().unwrap(), Some(b"end".as_slice()));
        assert_eq!(tokens.next().unwrap(), None);
    }
}
