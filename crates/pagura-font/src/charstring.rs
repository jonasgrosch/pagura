//! Charstring machinery shared by Type 1 charstrings (Adobe Type 1 Font
//! Format 6), Type 2 charstrings (TN 5177 3-4) and CFF DICT data (TN 5176
//! 4): the operand stack with its format ceiling, number encoding, call
//! flow and CFF2 blending.
use crate::read::{limit, syntax};
use pagura_core::bytes::Cursor;
use pagura_core::{Position, Result};

/// How an interpreted charstring or subroutine ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    /// A subroutine returned to its caller.
    Return,
    /// The glyph ended (endchar, seac, or a repaired unknown operator).
    End,
}

/// An operand stack holding at most `N` operands, its format's ceiling
/// (24 for Type 1, 48 for Type 2 and CFF DICTs, 513 for CFF2), so a stack
/// is no larger than its format allows. Every value on it is finite:
/// arithmetic results are checked as they are pushed, and blends check the
/// values they write in place.
#[derive(Debug, Clone)]
pub(crate) struct Operands<const N: usize> {
    values: [f64; N],
    len: usize,
}

impl<const N: usize> Operands<N> {
    pub(crate) const fn new() -> Self {
        Self {
            values: [0.0; N],
            len: 0,
        }
    }

    pub(crate) fn push(&mut self, value: f64, at: Position) -> Result<()> {
        if !value.is_finite() {
            return Err(syntax(at, "nonfinite charstring operand"));
        }
        let slot = self
            .values
            .get_mut(self.len)
            .ok_or_else(|| syntax(at, "operand stack exceeds the format limit"))?;
        *slot = value;
        self.len += 1;
        Ok(())
    }

    pub(crate) fn pop(&mut self, at: Position) -> Result<f64> {
        let top = *self
            .as_slice()
            .last()
            .ok_or_else(|| syntax(at, "charstring stack underflow"))?;
        self.len -= 1;
        Ok(top)
    }

    pub(crate) fn as_slice(&self) -> &[f64] {
        &self.values[..self.len]
    }

    pub(crate) fn as_mut_slice(&mut self) -> &mut [f64] {
        &mut self.values[..self.len]
    }

    pub(crate) const fn len(&self) -> usize {
        self.len
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) const fn clear(&mut self) {
        self.len = 0;
    }

    /// Drops the operands above the bottom `len`.
    pub(crate) const fn truncate(&mut self, len: usize) {
        if len < self.len {
            self.len = len;
        }
    }

    /// The operands, which must number exactly `count`.
    pub(crate) fn exactly(&self, count: usize, at: Position) -> Result<&[f64]> {
        if self.len == count {
            Ok(self.as_slice())
        } else {
            Err(syntax(at, "invalid charstring operand count"))
        }
    }

    /// Removes and returns the bottom operand: a Type 2 width preceding
    /// the operator's own operands (TN 5177 3.1).
    pub(crate) fn shift(&mut self) -> Option<f64> {
        let first = *self.as_slice().first()?;
        self.values.copy_within(1..self.len, 0);
        self.len -= 1;
        Some(first)
    }

    /// Replaces the top `count × (1 + coefficients)` operands with `count`
    /// blended values: each default value plus its deltas weighted by the
    /// region scalars (OpenType CFF2 `blend`; also in CFF2 DICTs).
    pub(crate) fn blend(&mut self, count: usize, coefficients: &[f64], at: Position) -> Result<()> {
        let size = count
            .checked_mul(coefficients.len() + 1)
            .ok_or_else(|| limit(at, "CFF2 blend operand count overflow"))?;
        if count == 0 || size > self.len {
            return Err(syntax(at, "invalid CFF2 blend operands"));
        }
        let start = self.len - size;
        let (defaults, deltas) = self.values[start..self.len].split_at_mut(count);
        for (value, deltas) in defaults
            .iter_mut()
            .zip(deltas.chunks_exact(coefficients.len().max(1)))
        {
            let blended = coefficients
                .iter()
                .zip(deltas)
                .fold(*value, |sum, (k, delta)| sum + k * delta);
            if !blended.is_finite() {
                return Err(syntax(at, "nonfinite CFF2 blend result"));
            }
            *value = blended;
        }
        self.len = start + count;
        Ok(())
    }
}

/// A nonnegative integral operand used as an index, count or offset.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is checked to be an integral u32 first"
)]
pub(crate) fn integer(n: f64, at: Position) -> Result<usize> {
    if !n.is_finite() || n < 0.0 || n > f64::from(u32::MAX) || n.fract() != 0.0 {
        return Err(syntax(at, "invalid charstring integer"));
    }
    Ok(pagura_core::convert::u32_to_usize(n as u32))
}

/// Decodes the number encodings that Type 1, Type 2 and CFF DICT data share,
/// introduced by `byte` in 32..=254 (Type 1 6.2; TN 5177 3.2; TN 5176
/// Table 3). Other bytes are format-specific and yield `None`.
pub(crate) fn shared_number(byte: u8, cursor: &mut Cursor<'_>) -> Result<Option<f64>> {
    Ok(Some(match byte {
        32..=246 => f64::from(i16::from(byte) - 139),
        247..=250 => f64::from((u16::from(byte) - 247) * 256 + u16::from(cursor.u8()?) + 108),
        251..=254 => -f64::from((u16::from(byte) - 251) * 256 + u16::from(cursor.u8()?) + 108),
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::{Operands, integer, shared_number};
    use pagura_core::bytes::Cursor;
    use pagura_core::{ErrorKind, Position};

    const AT: Position = Position::Buffer(9);

    #[test]
    fn the_stack_ceiling_and_finiteness_are_format_errors() {
        let mut stack = Operands::<2>::new();
        stack.push(1.0, AT).unwrap();
        stack.push(2.0, AT).unwrap();
        let error = stack.push(3.0, AT).unwrap_err();
        assert_eq!((error.kind, error.position), (ErrorKind::Syntax, AT));
        assert_eq!(stack.shift(), Some(1.0));
        assert_eq!(stack.as_slice(), [2.0]);
        assert_eq!(
            stack.push(f64::NAN, AT).unwrap_err().kind,
            ErrorKind::Syntax
        );
        assert_eq!(stack.pop(AT).unwrap(), 2.0);
        assert_eq!(stack.pop(AT).unwrap_err().kind, ErrorKind::Syntax);
    }

    #[test]
    fn blends_keep_preceding_operands_and_check_their_size() {
        let mut stack = Operands::<48>::new();
        for value in [7.0, 5.0, 10.0, 20.0, 30.0] {
            stack.push(value, AT).unwrap();
        }
        // Two blended values need six operands; one needs three:
        // 10 + 0 × 20 + 1 × 30, below the untouched 7 and 5.
        stack.blend(2, &[0.0, 1.0], AT).unwrap_err();
        stack.blend(1, &[0.0, 1.0], AT).unwrap();
        assert_eq!(stack.as_slice(), [7.0, 5.0, 40.0]);
        assert_eq!(
            stack.blend(0, &[1.0], AT).unwrap_err().kind,
            ErrorKind::Syntax
        );
        assert_eq!(
            stack.blend(usize::MAX, &[1.0], AT).unwrap_err().kind,
            ErrorKind::Limit
        );
    }

    #[test]
    fn shared_numbers_cover_the_one_and_two_byte_ranges() {
        let decode = |bytes: &[u8]| {
            let mut cursor = Cursor::new(&bytes[1..]);
            shared_number(bytes[0], &mut cursor).unwrap()
        };
        assert_eq!(decode(&[32]), Some(-107.0));
        assert_eq!(decode(&[246]), Some(107.0));
        assert_eq!(decode(&[247, 0]), Some(108.0));
        assert_eq!(decode(&[250, 255]), Some(1131.0));
        assert_eq!(decode(&[251, 0]), Some(-108.0));
        assert_eq!(decode(&[254, 255]), Some(-1131.0));
        assert_eq!(decode(&[28, 0, 0]), None);
        assert_eq!(integer(3.0, AT).unwrap(), 3);
        for invalid in [-1.0, 0.5, f64::INFINITY, 5e9] {
            assert_eq!(integer(invalid, AT).unwrap_err().kind, ErrorKind::Syntax);
        }
    }
}
