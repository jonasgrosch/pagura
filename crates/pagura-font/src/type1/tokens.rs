//! Bounded lexical extraction of static Type 1 dictionary entries.
use super::aliases::Aliases;
use crate::budget::Work;
use crate::features;
use crate::read::{limit, syntax, unsupported};
use pagura_core::{Position, Result};
use pagura_raster::Transform;

/// Type 1 chapter 10: tokens and RD strings are at most 65,535 bytes.
const MAX_TOKEN_BYTES: usize = 65_535;

/// Tokens of one Type 1 buffer (cleartext, plaintext or a procedure body),
/// positioned in that buffer and charged to the parse's work budget.
pub(super) struct Tokens<'a, 'w, 'd> {
    data: &'a [u8],
    pos: usize,
    start: usize,
    work: &'w mut Work<'d>,
}

impl<'a, 'w, 'd> Tokens<'a, 'w, 'd> {
    pub(super) const fn new(data: &'a [u8], work: &'w mut Work<'d>) -> Self {
        Self {
            data,
            pos: 0,
            start: 0,
            work,
        }
    }

    /// The offset of the next unread byte.
    pub(super) const fn offset(&self) -> usize {
        self.pos
    }

    /// The offset where the last token returned started.
    pub(super) const fn token_start(&self) -> usize {
        self.start
    }

    pub(super) const fn position(&self) -> Position {
        Position::Buffer(self.pos)
    }

    pub(super) fn work(&mut self) -> &mut Work<'d> {
        self.work
    }

    /// The next token, skipping procedures and strings.
    pub(super) fn token(&mut self) -> Result<Option<&'a [u8]>> {
        loop {
            let token = self.raw()?;
            if token.is_none_or(|t| !matches!(t.first(), Some(b'{' | b'('))) {
                return Ok(token);
            }
        }
    }

    /// The next token, including a balanced procedure or string.
    pub(super) fn raw(&mut self) -> Result<Option<&'a [u8]>> {
        loop {
            while self.data.get(self.pos).is_some_and(|&b| white(b)) {
                self.pos += 1;
            }
            if self.data.get(self.pos) == Some(&b'%') {
                self.skip_comment();
                continue;
            }
            let Some(&byte) = self.data.get(self.pos) else {
                return Ok(None);
            };
            let at = self.position();
            self.work.work(1, at)?;
            let start = self.pos;
            self.start = start;
            self.pos += 1;
            match byte {
                b'{' => self.balanced(b'{', b'}')?,
                b'(' => self.balanced(b'(', b')')?,
                b'[' | b']' | b'<' | b'>' | b'}' => {}
                _ => {
                    while self
                        .data
                        .get(self.pos)
                        .is_some_and(|&b| !white(b) && !b"()[]{}<>/%".contains(&b))
                    {
                        self.pos += 1;
                    }
                }
            }
            let token = self.data.get(start..self.pos).unwrap_or_default();
            if !matches!(byte, b'{' | b'(') {
                if token.len() > MAX_TOKEN_BYTES {
                    return Err(syntax(at, "Type 1 token exceeds 65,535 bytes"));
                }
                self.work.string(token.len(), at)?;
            }
            return Ok(Some(token));
        }
    }

    fn skip_comment(&mut self) {
        while self
            .data
            .get(self.pos)
            .is_some_and(|b| !matches!(b, b'\r' | b'\n'))
        {
            self.pos += 1;
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
                self.skip_comment();
                continue;
            }
            if byte == open {
                depth += 1;
                self.work.depth(depth, usize::MAX, self.position())?;
            }
            if byte == close {
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
        }
        Err(syntax(
            self.position(),
            "unterminated Type 1 string or procedure",
        ))
    }

    /// The body of a procedure token, or `None` (consuming nothing) if the
    /// next token is not a procedure.
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
        let (pos, start) = (self.pos, self.start);
        let token = self.token()?;
        (self.pos, self.start) = (pos, start);
        Ok(token)
    }

    pub(super) fn required(&mut self) -> Result<&'a [u8]> {
        self.token()?
            .ok_or_else(|| syntax(self.position(), "truncated Type 1 dictionary"))
    }

    pub(super) fn integer(&mut self) -> Result<usize> {
        let token = self.required()?;
        integer(token, self.position())
    }

    /// The bytes of an RD string and the offset of their first byte.
    pub(super) fn binary(&mut self, aliases: &Aliases) -> Result<(&'a [u8], usize)> {
        let size = self.integer()?;
        if size > MAX_TOKEN_BYTES {
            return Err(syntax(
                self.position(),
                "Type 1 RD string exceeds 65,535 bytes",
            ));
        }
        self.work.decoded(size, self.position())?;
        if !aliases.reader(self.required()?) {
            return Err(unsupported(
                features::TYPE1_POSTSCRIPT,
                self.position(),
                "Type 1 binary reader is not implemented",
            ));
        }
        if !self.data.get(self.pos).is_some_and(|&b| white(b)) {
            return Err(syntax(self.position(), "Type 1 RD needs a delimiter"));
        }
        self.pos += 1;
        let start = self.pos;
        let end = start
            .checked_add(size)
            .ok_or_else(|| limit(self.position(), "Type 1 binary size overflow"))?;
        let bytes = self
            .data
            .get(start..end)
            .ok_or_else(|| syntax(self.position(), "truncated Type 1 binary data"))?;
        self.pos = end;
        Ok((bytes, start))
    }

    pub(super) fn matrix(&mut self) -> Result<Transform> {
        while self.data.get(self.pos).is_some_and(|&b| white(b)) {
            self.pos += 1;
        }
        let close = if self.data.get(self.pos) == Some(&b'{') {
            self.pos += 1;
            b"}".as_slice()
        } else if self.required()? == b"[" {
            b"]".as_slice()
        } else {
            return Err(unsupported(
                features::TYPE1_POSTSCRIPT,
                self.position(),
                "computed Type 1 FontMatrix is not implemented",
            ));
        };
        let mut numbers = [0.0; 6];
        for n in &mut numbers {
            *n = std::str::from_utf8(self.required()?)
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|n| n.is_finite())
                .ok_or_else(|| syntax(self.position(), "invalid Type 1 matrix number"))?;
        }
        if self.required()? != close {
            return Err(syntax(self.position(), "invalid Type 1 matrix size"));
        }
        let [a, b, c, d, e, f] = numbers;
        let matrix = Transform::new(a, b, c, d, e, f);
        if matrix.determinant() == 0.0 {
            return Err(syntax(self.position(), "singular Type 1 FontMatrix"));
        }
        Ok(matrix)
    }
}

pub(super) fn integer(bytes: &[u8], at: Position) -> Result<usize> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| syntax(at, "invalid Type 1 integer"))
}

/// PostScript white-space characters (PLRM 3.2.2): NUL, tab, line feed,
/// form feed, carriage return and space.
const fn white(byte: u8) -> bool {
    matches!(byte, b'\0' | b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

#[cfg(test)]
mod tests {
    use super::Tokens;
    use crate::budget::{FontBudget, Work};
    use pagura_core::{Diagnostics, ErrorKind, Limits, Recovery};

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
        assert_eq!(tokens.token().unwrap(), Some(b"end".as_slice()));
        assert_eq!(tokens.token_start(), 70002);
        assert_eq!(tokens.token().unwrap(), None);
    }

    #[test]
    fn tokens_are_work_and_long_names_are_budgeted() {
        let limits = Limits {
            font_work: 2,
            ..Limits::default()
        };
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(&limits), &mut diagnostics);
        let mut tokens = Tokens::new(b"/a /b /c", &mut work);
        tokens.required().unwrap();
        assert_eq!(tokens.peek().unwrap(), Some(b"/b".as_slice()));
        assert_eq!(tokens.token().unwrap_err().kind, ErrorKind::Limit);
        let limits = Limits {
            string_bytes: 2,
            ..Limits::default()
        };
        let mut work = Work::new(FontBudget::new(&limits), &mut diagnostics);
        let mut tokens = Tokens::new(b"/abc", &mut work);
        assert_eq!(tokens.token().unwrap_err().kind, ErrorKind::Limit);
    }
}
