//! CFF DICT evaluation (TN 5176 section 4); CFF2 extensions await primary review.
use crate::budget::Work;
use crate::read::{bad, here};
use pagura_core::bytes::Cursor;
use pagura_core::{Position, Result};
use pagura_raster::Transform;
use std::collections::BTreeMap;

mod blues;
mod validation;

/// DICT operand stack ceilings: TN 5176 Appendix B (48) and CFF2 (513).
const MAX_OPERANDS: usize = 48;
const MAX_CFF2_OPERANDS: usize = 513;
/// The longest real accepted, in characters. TN 5176 sets no limit; the
/// shortest round-trip form of any `f64` is far shorter.
const MAX_REAL_CHARS: usize = 64;

#[derive(Debug)]
pub(super) struct Dict {
    entries: BTreeMap<u16, Entry>,
    at: usize,
    blue_order_fault: Option<usize>,
}
#[derive(Debug)]
struct Entry {
    values: Vec<f64>,
    at: usize,
}
impl Dict {
    pub(super) fn values(&self, op: u16) -> Option<&[f64]> {
        self.entries.get(&op).map(|entry| entry.values.as_slice())
    }
    pub(super) fn has(&self, op: u16) -> bool {
        self.entries.contains_key(&op)
    }
    pub(super) fn number(&self, op: u16, default: f64) -> Result<f64> {
        match self.values(op) {
            None => Ok(default),
            Some([n]) => Ok(*n),
            _ => Err(bad(self.at, "invalid CFF DICT operand count")),
        }
    }
    pub(super) fn offset(&self, op: u16, default: Option<usize>) -> Result<usize> {
        match self.values(op) {
            None => default.ok_or_else(|| bad(self.at, "missing CFF DICT offset")),
            Some([n]) => integer(*n, self.at),
            _ => Err(bad(self.at, "invalid CFF offset operands")),
        }
    }
    pub(super) fn matrix(&self, default: Transform) -> Result<Transform> {
        match self.values(0x0c07) {
            None => Ok(default),
            Some([a, b, c, d, e, f]) => {
                let m = Transform::new(*a, *b, *c, *d, *e, *f);
                if m.determinant() == 0.0 {
                    return Err(bad(self.at, "singular CFF font matrix"));
                }
                Ok(m)
            }
            _ => Err(bad(self.at, "invalid CFF font matrix")),
        }
    }
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "range and integral value are checked before conversion"
)]
pub(super) fn integer(n: f64, at: usize) -> Result<usize> {
    if !n.is_finite() || n < 0.0 || n > f64::from(u32::MAX) || n.fract() != 0.0 {
        return Err(bad(at, "invalid CFF integer"));
    }
    usize::try_from(n as u32)
        .map_err(|_| crate::read::limit(Position::Buffer(at), "CFF integer overflow"))
}

pub(super) fn number(r: &mut Cursor<'_>, byte: u8, real: bool) -> Result<f64> {
    Ok(match byte {
        28 => f64::from(r.i16()?),
        29 if real => f64::from(r.i32()?),
        30 if real => return real_number(r),
        32..=246 => f64::from(i16::from(byte) - 139),
        247..=250 => f64::from((u16::from(byte) - 247) * 256 + u16::from(r.u8()?) + 108),
        251..=254 => -f64::from((u16::from(byte) - 251) * 256 + u16::from(r.u8()?) + 108),
        255 if !real => f64::from(r.i32()?) / 65536.0,
        _ => return Err(bad(here(r), "invalid CFF number")),
    })
}
fn real_number(r: &mut Cursor<'_>) -> Result<f64> {
    let mut text = String::new();
    loop {
        let byte = r.u8()?;
        for (i, nibble) in [byte >> 4, byte & 15].into_iter().enumerate() {
            if text.len() > MAX_REAL_CHARS {
                return Err(bad(here(r), "CFF real exceeds 64 characters"));
            }
            match nibble {
                0..=9 => text.push(char::from(b'0' + nibble)),
                10 => text.push('.'),
                11 => text.push('E'),
                12 => text.push_str("E-"),
                14 => text.push('-'),
                15 => {
                    if i == 0 && byte & 15 != 15 {
                        return Err(bad(here(r) - 1, "invalid CFF real padding"));
                    }
                    return text
                        .parse::<f64>()
                        .ok()
                        .filter(|n| n.is_finite())
                        .ok_or_else(|| bad(here(r), "invalid CFF real"));
                }
                _ => return Err(bad(here(r), "reserved CFF real nibble")),
            }
        }
    }
}
pub(super) fn parse(r: Cursor<'_>, work: &mut Work<'_>) -> Result<Dict> {
    parse_inner(r, work, None, false)
}

pub(super) fn parse_private(
    r: Cursor<'_>,
    work: &mut Work<'_>,
    variations: &[Vec<f64>],
) -> Result<Dict> {
    parse_inner(r, work, Some(variations), true)
}

pub(super) fn parse_cff2(r: Cursor<'_>, work: &mut Work<'_>) -> Result<Dict> {
    parse_inner(r, work, None, true)
}

fn parse_inner(
    mut r: Cursor<'_>,
    work: &mut Work<'_>,
    variations: Option<&[Vec<f64>]>,
    cff2: bool,
) -> Result<Dict> {
    let start = here(&r);
    let mut entries = BTreeMap::new();
    let mut blue_order_fault = None;
    let mut stack = Vec::new();
    let mut vsindex = 0;
    let ceiling = if cff2 {
        MAX_CFF2_OPERANDS
    } else {
        MAX_OPERANDS
    };
    while !r.at_end() {
        let at = here(&r);
        work.work(1, Position::Buffer(at))?;
        let byte = r.u8()?;
        if byte >= 28 {
            if stack.len() == ceiling {
                return Err(bad(at, "operand stack exceeds the format limit"));
            }
            stack.push(number(&mut r, byte, true)?);
        } else {
            let op = if byte == 12 {
                0x0c00 | u16::from(r.u8()?)
            } else {
                u16::from(byte)
            };
            if let Some(variations) = variations {
                if op == 22 {
                    let [value] = stack.as_slice() else {
                        return Err(bad(here(&r), "invalid CFF2 DICT vsindex operands"));
                    };
                    vsindex = integer(*value, here(&r))?;
                    if vsindex >= variations.len() {
                        return Err(bad(here(&r), "CFF2 DICT vsindex out of range"));
                    }
                    work.entries(1, Position::Buffer(at))?;
                    entries.insert(
                        22,
                        Entry {
                            values: std::mem::take(&mut stack),
                            at,
                        },
                    );
                    continue;
                }
                if op == 23 {
                    blend(&mut stack, vsindex, variations, here(&r))?;
                    continue;
                }
            }
            if !cff2 && reserved(op) {
                work.repair(
                    crate::repairs::CFF_DICT_OPERATOR,
                    "reserved CFF DICT operator ignored",
                    Position::Buffer(at),
                )?;
                stack.clear();
                continue;
            }
            if matches!(op, 7 | 9) && !entries.contains_key(&(op - 1)) {
                blue_order_fault.get_or_insert(here(&r) - 1);
            }
            work.entries(1, Position::Buffer(at))?;
            if entries
                .insert(
                    op,
                    Entry {
                        values: std::mem::take(&mut stack),
                        at,
                    },
                )
                .is_some()
            {
                return Err(bad(here(&r), "duplicate CFF DICT operator"));
            }
        }
    }
    if !stack.is_empty() {
        return Err(bad(here(&r), "unterminated CFF DICT operands"));
    }
    Ok(Dict {
        entries,
        at: start,
        blue_order_fault,
    })
}

// TN 5176 Appendix H identifies operator slots with no defined semantics.
// Reserved number bytes 31/255 remain errors: no payload length is defined.
fn reserved(op: u16) -> bool {
    matches!(op, 22..=27 | 0x0c0f..=0x0c10 | 0x0c18..=0x0c1d | 0x0c27..=0x0cff)
}

// DICT delta-encoded arrays are accumulated by their consumers, after blending.
fn blend(stack: &mut Vec<f64>, vsindex: usize, variations: &[Vec<f64>], at: usize) -> Result<()> {
    let count = integer(
        stack
            .pop()
            .ok_or_else(|| bad(at, "missing CFF2 DICT blend count"))?,
        at,
    )?;
    let coefficients = variations
        .get(vsindex)
        .ok_or_else(|| bad(at, "missing CFF2 DICT variation data"))?;
    let size = count
        .checked_mul(coefficients.len() + 1)
        .ok_or_else(|| crate::read::limit(Position::Buffer(at), "CFF2 DICT blend size overflow"))?;
    if count == 0 || size > stack.len() {
        return Err(bad(at, "CFF2 DICT blend stack underflow"));
    }
    let first = stack.len() - size;
    for i in 0..count {
        let mut value = stack[first + i];
        for (j, coefficient) in coefficients.iter().enumerate() {
            value += coefficient * stack[first + count + i * coefficients.len() + j];
        }
        if !value.is_finite() {
            return Err(bad(at, "nonfinite CFF2 DICT blend result"));
        }
        stack[first + i] = value;
    }
    stack.truncate(first + count);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Dict, parse, parse_private};
    use crate::budget::{FontBudget, Work};
    use crate::read::cursor;
    use pagura_core::{Diagnostics, ErrorKind, Limits, Position, Recovery};

    fn private(data: &[u8], variations: &[Vec<f64>], limits: &Limits) -> pagura_core::Result<Dict> {
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(limits), &mut diagnostics);
        parse_private(cursor(data).with_base(100), &mut work, variations)
    }

    #[test]
    fn blends_preserve_preceding_operands_and_feed_dict_operators() {
        // The first coefficient is zero at default, the second one is one.
        let data = [144, 149, 159, 169, 140, 23, 6]; // 5,10,20,30,1 blend BlueValues
        let dict = private(&data, &[vec![0.0, 1.0]], &Limits::default()).unwrap();
        assert_eq!(dict.values(6), Some([5.0, 40.0].as_slice()));
        let small = Limits {
            font_entries: 1,
            ..Limits::default()
        };
        let error = private(&[139, 19, 139, 22], &[vec![1.0]], &small).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        for malformed in [&[140, 23][..], &[140, 22][..], &[139, 23][..]] {
            let e = private(malformed, &[vec![1.0]], &Limits::default()).unwrap_err();
            assert_eq!(e.kind, ErrorKind::Syntax);
            assert!(matches!(e.position, Position::Buffer(_)));
        }
    }

    #[test]
    fn the_operand_stack_ceiling_is_a_format_error() {
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(&Limits::default()), &mut diagnostics);
        let mut stack = vec![139; 49];
        stack.push(6);
        let error = parse(cursor(&stack), &mut work).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        stack.truncate(48);
        stack.push(6);
        parse(cursor(&stack), &mut work).unwrap();
    }
}
