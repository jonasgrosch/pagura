//! CFF DICT evaluation (TN 5176 section 4); CFF2 extensions await primary review.
use crate::read::{Read, bad, cap};
use pagura_core::{Limits, Result};
use pagura_raster::Transform;
use std::collections::BTreeMap;

mod blues;
mod validation;

pub(super) struct Dict {
    entries: BTreeMap<u16, Vec<f64>>,
    at: usize,
    blue_order_fault: Option<usize>,
}
impl Dict {
    pub(super) fn check(&self, private: bool, strings: usize) -> Result<()> {
        self.validate(private, strings)
    }
    pub fn values(&self, op: u16) -> Option<&[f64]> {
        self.entries.get(&op).map(Vec::as_slice)
    }
    pub fn has(&self, op: u16) -> bool {
        self.entries.contains_key(&op)
    }
    pub fn number(&self, op: u16, default: f64) -> Result<f64> {
        match self.values(op) {
            None => Ok(default),
            Some([n]) => Ok(*n),
            _ => Err(bad(self.at, "invalid CFF DICT operand count")),
        }
    }
    pub fn offset(&self, op: u16, default: Option<usize>) -> Result<usize> {
        match self.values(op) {
            None => default.ok_or_else(|| bad(self.at, "missing CFF DICT offset")),
            Some([n]) => integer(*n, self.at),
            _ => Err(bad(self.at, "invalid CFF offset operands")),
        }
    }
    pub fn matrix(&self, default: Transform) -> Result<Transform> {
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
    usize::try_from(n as u32).map_err(|_| crate::read::limit(at, "CFF integer overflow"))
}

pub(super) fn number(r: &mut Read<'_>, byte: u8, real: bool) -> Result<f64> {
    Ok(match byte {
        28 => f64::from(r.i16()?),
        29 if real => f64::from(r.i32()?),
        30 if real => return real_number(r),
        32..=246 => f64::from(i16::from(byte) - 139),
        247..=250 => f64::from((u16::from(byte) - 247) * 256 + u16::from(r.u8()?) + 108),
        251..=254 => -f64::from((u16::from(byte) - 251) * 256 + u16::from(r.u8()?) + 108),
        255 if !real => f64::from(r.i32()?) / 65536.0,
        _ => return Err(bad(r.at(), "invalid CFF number")),
    })
}
fn real_number(r: &mut Read<'_>) -> Result<f64> {
    let mut text = String::new();
    loop {
        let byte = r.u8()?;
        for (i, nibble) in [byte >> 4, byte & 15].into_iter().enumerate() {
            cap(text.len(), 64, r.at())?;
            match nibble {
                0..=9 => text.push(char::from(b'0' + nibble)),
                10 => text.push('.'),
                11 => text.push('E'),
                12 => text.push_str("E-"),
                14 => text.push('-'),
                15 => {
                    if i == 0 && byte & 15 != 15 {
                        return Err(bad(r.at() - 1, "invalid CFF real padding"));
                    }
                    return text
                        .parse::<f64>()
                        .ok()
                        .filter(|n| n.is_finite())
                        .ok_or_else(|| bad(r.at(), "invalid CFF real"));
                }
                _ => return Err(bad(r.at(), "reserved CFF real nibble")),
            }
        }
    }
}
pub(super) fn parse(r: Read<'_>, limits: &Limits) -> Result<Dict> {
    parse_inner(r, limits, None, false)
}

pub(super) fn parse_private(r: Read<'_>, limits: &Limits, variations: &[Vec<f64>]) -> Result<Dict> {
    parse_inner(r, limits, Some(variations), true)
}

pub(super) fn parse_cff2(r: Read<'_>, limits: &Limits) -> Result<Dict> {
    parse_inner(r, limits, None, true)
}

fn parse_inner(
    mut r: Read<'_>,
    limits: &Limits,
    variations: Option<&[Vec<f64>]>,
    cff2: bool,
) -> Result<Dict> {
    let mut entries = BTreeMap::new();
    let mut blue_order_fault = None;
    let mut stack = Vec::new();
    let mut work = 0;
    let mut vsindex = 0;
    while r.pos < r.data.len() {
        work += 1;
        cap(work, limits.operations, r.at())?;
        let byte = r.u8()?;
        if byte >= 28 {
            cap(
                stack.len() + 1,
                if cff2 { 513 } else { 48 }.min(limits.value_nodes),
                r.at(),
            )?;
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
                        return Err(bad(r.at(), "invalid CFF2 DICT vsindex operands"));
                    };
                    vsindex = integer(*value, r.at())?;
                    if vsindex >= variations.len() {
                        return Err(bad(r.at(), "CFF2 DICT vsindex out of range"));
                    }
                    entries.insert(22, std::mem::take(&mut stack));
                    cap(entries.len(), limits.value_nodes, r.at())?;
                    continue;
                }
                if op == 23 {
                    blend(&mut stack, vsindex, variations, r.at())?;
                    continue;
                }
            }
            if !cff2 && matches!(op, 22..=27) {
                return Err(bad(r.at(), "reserved CFF DICT operator"));
            }
            if matches!(op, 7 | 9) && !entries.contains_key(&(op - 1)) {
                blue_order_fault.get_or_insert(r.at() - 1);
            }
            if entries.insert(op, std::mem::take(&mut stack)).is_some() {
                return Err(bad(r.at(), "duplicate CFF DICT operator"));
            }
            cap(entries.len(), limits.value_nodes, r.at())?;
        }
    }
    if !stack.is_empty() {
        return Err(bad(r.at(), "unterminated CFF DICT operands"));
    }
    Ok(Dict {
        entries,
        at: r.base,
        blue_order_fault,
    })
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
        .ok_or_else(|| crate::read::limit(at, "CFF2 DICT blend size overflow"))?;
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
            return Err(crate::read::limit(at, "nonfinite CFF2 DICT blend result"));
        }
        stack[first + i] = value;
    }
    stack.truncate(first + count);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Read, parse_private};
    use pagura_core::{ErrorKind, Limits, Position};
    #[test]
    fn blends_preserve_preceding_operands_and_feed_dict_operators() {
        // The first coefficient is zero at default, the second one is one.
        let data = [144, 149, 159, 169, 140, 23, 6]; // 5,10,20,30,1 blend BlueValues
        let dict =
            parse_private(Read::new(&data, 100), &Limits::default(), &[vec![0.0, 1.0]]).unwrap();
        assert_eq!(dict.values(6), Some([5.0, 40.0].as_slice()));
        let small = Limits {
            value_nodes: 1,
            ..Limits::default()
        };
        assert!(parse_private(Read::new(&[139, 19, 139, 22], 0), &small, &[vec![1.0]]).is_err());
        for malformed in [&[140, 23][..], &[140, 22][..], &[139, 23][..]] {
            let e = parse_private(Read::new(malformed, 20), &Limits::default(), &[vec![1.0]])
                .err()
                .unwrap();
            assert_eq!(e.kind, ErrorKind::Syntax);
            assert!(matches!(e.position, Position::Buffer(_)));
        }
    }
}
