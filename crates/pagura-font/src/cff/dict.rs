//! CFF DICT numbers and operators, independently decoded from TN 5176.
use crate::read::{Read, bad, cap};
use pagura_core::{Limits, Result};
use pagura_raster::Transform;
use std::collections::BTreeMap;

pub(super) struct Dict {
    entries: BTreeMap<u16, Vec<f64>>,
    at: usize,
}
impl Dict {
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
    usize::try_from(n as u32).map_err(|_| bad(at, "CFF integer overflow"))
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
        for nibble in [byte >> 4, byte & 15] {
            cap(text.len(), 64, r.at())?;
            match nibble {
                0..=9 => text.push(char::from(b'0' + nibble)),
                10 => text.push('.'),
                11 => text.push('E'),
                12 => text.push_str("E-"),
                14 => text.push('-'),
                15 => {
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
pub(super) fn parse(mut r: Read<'_>, limits: &Limits) -> Result<Dict> {
    let mut entries = BTreeMap::new();
    let mut stack = Vec::new();
    let mut work = 0;
    while r.pos < r.data.len() {
        work += 1;
        cap(work, limits.operations, r.at())?;
        let byte = r.u8()?;
        if byte >= 28 {
            cap(stack.len() + 1, 513.min(limits.value_nodes), r.at())?;
            stack.push(number(&mut r, byte, true)?);
        } else {
            let op = if byte == 12 {
                0x0c00 | u16::from(r.u8()?)
            } else {
                u16::from(byte)
            };
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
    })
}
