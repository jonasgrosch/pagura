//! CFF DICT evaluation (TN 5176 section 4), with current OpenType CFF2 namespaces.
use crate::budget::Work;
use crate::charstring::{Operands, integer, shared_number};
use crate::read::syntax;
use crate::repairs;
use pagura_core::bytes::Cursor;
use pagura_core::{Position, Result};
use pagura_raster::Transform;
use std::collections::BTreeMap;

mod blues;
mod operator;
mod validation;

use operator::ESCAPE;
pub(super) use operator::Operator;
pub(super) use validation::Domain;

/// DICT operand stack ceilings: TN 5176 Appendix B (48) and CFF2 (513).
const MAX_OPERANDS: usize = 48;
const MAX_CFF2_OPERANDS: usize = 513;

/// Number encodings used only in DICT data (TN 5176 Table 3).
const SHORT_INT: u8 = 28;
const LONG_INT: u8 = 29;
const REAL: u8 = 30;

/// Nibbles of a real number (TN 5176 Table 5); 13 is reserved.
const DECIMAL_POINT: u8 = 0xa;
const EXPONENT: u8 = 0xb;
const NEGATIVE_EXPONENT: u8 = 0xc;
const MINUS: u8 = 0xe;
const END_OF_NUMBER: u8 = 0xf;
/// The longest real accepted, in characters. TN 5176 sets no limit; the
/// shortest round-trip form of any `f64` is far shorter.
const MAX_REAL_CHARS: usize = 64;

/// Which DICT grammar applies.
#[derive(Debug, Clone, Copy)]
pub(super) enum Kind<'a> {
    /// A CFF DICT: reserved operators are reported repairs.
    Cff,
    /// A CFF2 Top DICT.
    Cff2Top,
    /// A current CFF2 Font DICT: only the Private DICT offset is recognized.
    Cff2Font,
    /// A CFF2 Private DICT, whose `vsindex` and `blend` operators use the
    /// variation store's region scalars.
    Cff2Private(&'a [Vec<f64>]),
}

impl Kind<'_> {
    /// CFF2's operator namespaces are specific to each DICT type. An operator
    /// defined only in another namespace is still unknown here (OpenType CFF2,
    /// Stack-based decoding and the three DICT operator summary tables).
    const fn recognizes(self, op: Operator) -> bool {
        match self {
            Self::Cff => true,
            Self::Cff2Top => matches!(
                op,
                Operator::CHAR_STRINGS
                    | Operator::VSTORE
                    | Operator::FD_ARRAY
                    | Operator::FD_SELECT
                    | Operator::FONT_MATRIX
            ),
            Self::Cff2Font => matches!(op, Operator::PRIVATE),
            Self::Cff2Private(_) => matches!(
                op,
                Operator::SUBRS
                    | Operator::VSINDEX
                    | Operator::BLEND
                    | Operator::BLUE_VALUES
                    | Operator::OTHER_BLUES
                    | Operator::FAMILY_BLUES
                    | Operator::FAMILY_OTHER_BLUES
                    | Operator::BLUE_SCALE
                    | Operator::BLUE_SHIFT
                    | Operator::BLUE_FUZZ
                    | Operator::STD_HW
                    | Operator::STD_VW
                    | Operator::STEM_SNAP_H
                    | Operator::STEM_SNAP_V
                    | Operator::LANGUAGE_GROUP
                    | Operator::EXPANSION_FACTOR
            ),
        }
    }
}

#[derive(Debug)]
pub(super) struct Dict {
    entries: BTreeMap<Operator, Entry>,
    at: Position,
    blue_order_fault: Option<Position>,
}

#[derive(Debug)]
struct Entry {
    values: Vec<f64>,
    at: Position,
}

impl Dict {
    pub(super) fn parse(r: Cursor<'_>, kind: Kind<'_>, work: &mut Work<'_>) -> Result<Self> {
        match kind {
            Kind::Cff => Self::parse_with::<MAX_OPERANDS>(r, kind, work),
            Kind::Cff2Top | Kind::Cff2Font | Kind::Cff2Private(_) => {
                Self::parse_with::<MAX_CFF2_OPERANDS>(r, kind, work)
            }
        }
    }

    fn parse_with<const N: usize>(
        mut r: Cursor<'_>,
        kind: Kind<'_>,
        work: &mut Work<'_>,
    ) -> Result<Self> {
        let start = r.position();
        let mut entries = BTreeMap::new();
        let mut blue_order_fault = None;
        let mut stack = Operands::<N>::new();
        let mut vsindex = 0;
        while !r.at_end() {
            let at = r.position();
            work.work(1, at)?;
            let byte = r.u8()?;
            if byte >= SHORT_INT {
                stack.push(number(byte, &mut r, at)?, at)?;
                continue;
            }
            let op = if byte == ESCAPE {
                Operator::escaped(r.u8()?)
            } else {
                Operator::one_byte(byte)
            };
            // Ignore unknown CFF2 operators before retaining entries or
            // validating arity. Numbers and the stack were already bounded.
            if !kind.recognizes(op) {
                stack.clear();
                continue;
            }
            if let Kind::Cff2Private(variations) = kind {
                if op == Operator::VSINDEX {
                    let [value] = stack.as_slice() else {
                        return Err(syntax(r.position(), "invalid CFF2 DICT vsindex operands"));
                    };
                    vsindex = integer(*value, r.position())?;
                    if vsindex >= variations.len() {
                        return Err(syntax(r.position(), "CFF2 DICT vsindex out of range"));
                    }
                    work.entries(1, at)?;
                    entries.insert(op, entry(&mut stack, at));
                    continue;
                }
                if op == Operator::BLEND {
                    let count = integer(stack.pop(at)?, at)?;
                    let coefficients = variations
                        .get(vsindex)
                        .ok_or_else(|| syntax(at, "missing CFF2 DICT variation data"))?;
                    stack.blend(count, coefficients, at)?;
                    continue;
                }
            }
            if matches!(kind, Kind::Cff) && op.is_reserved_in_cff() {
                work.repair(
                    repairs::CFF_DICT_OPERATOR,
                    "reserved CFF DICT operator ignored",
                    at,
                )?;
                stack.clear();
                continue;
            }
            if op
                .blue_predecessor()
                .is_some_and(|predecessor| !entries.contains_key(&predecessor))
            {
                blue_order_fault.get_or_insert(at);
            }
            work.entries(1, at)?;
            if entries.insert(op, entry(&mut stack, at)).is_some() {
                return Err(syntax(r.position(), "duplicate CFF DICT operator"));
            }
        }
        if !stack.is_empty() {
            return Err(syntax(r.position(), "unterminated CFF DICT operands"));
        }
        Ok(Self {
            entries,
            at: start,
            blue_order_fault,
        })
    }

    pub(super) fn values(&self, op: Operator) -> Option<&[f64]> {
        self.entries.get(&op).map(|entry| entry.values.as_slice())
    }

    pub(super) fn has(&self, op: Operator) -> bool {
        self.entries.contains_key(&op)
    }

    pub(super) fn number(&self, op: Operator, default: f64) -> Result<f64> {
        match self.values(op) {
            None => Ok(default),
            Some(&[n]) => Ok(n),
            _ => Err(syntax(self.at, "invalid CFF DICT operand count")),
        }
    }

    pub(super) fn offset(&self, op: Operator, default: Option<usize>) -> Result<usize> {
        match self.values(op) {
            None => default.ok_or_else(|| syntax(self.at, "missing CFF DICT offset")),
            Some(&[n]) => integer(n, self.at),
            _ => Err(syntax(self.at, "invalid CFF offset operands")),
        }
    }

    pub(super) fn matrix(&self, default: Transform) -> Result<Transform> {
        match self.values(Operator::FONT_MATRIX) {
            None => Ok(default),
            Some(&[a, b, c, d, e, f]) => {
                let m = Transform::new(a, b, c, d, e, f);
                if m.determinant() == 0.0 {
                    return Err(syntax(self.at, "singular CFF font matrix"));
                }
                Ok(m)
            }
            _ => Err(syntax(self.at, "invalid CFF font matrix")),
        }
    }
}

fn entry<const N: usize>(stack: &mut Operands<N>, at: Position) -> Entry {
    let values = stack.as_slice().to_vec();
    stack.clear();
    Entry { values, at }
}

fn number(byte: u8, r: &mut Cursor<'_>, at: Position) -> Result<f64> {
    Ok(match byte {
        SHORT_INT => f64::from(r.i16()?),
        LONG_INT => f64::from(r.i32()?),
        REAL => real(r)?,
        // 31 and 255 are reserved: no payload length is defined.
        _ => shared_number(byte, r)?.ok_or_else(|| syntax(at, "invalid CFF number"))?,
    })
}

fn real(r: &mut Cursor<'_>) -> Result<f64> {
    let mut text = [0_u8; MAX_REAL_CHARS];
    let mut len = 0;
    loop {
        let at = r.position();
        let byte = r.u8()?;
        for (i, nibble) in [byte >> 4, byte & 0xf].into_iter().enumerate() {
            let chars: &[u8] = match nibble {
                0..=9 => &[b'0' + nibble],
                DECIMAL_POINT => b".",
                EXPONENT => b"E",
                NEGATIVE_EXPONENT => b"E-",
                MINUS => b"-",
                END_OF_NUMBER => {
                    if i == 0 && byte & 0xf != END_OF_NUMBER {
                        return Err(syntax(at, "invalid CFF real padding"));
                    }
                    return std::str::from_utf8(text.get(..len).unwrap_or_default())
                        .ok()
                        .and_then(|s| s.parse::<f64>().ok())
                        .filter(|n| n.is_finite())
                        .ok_or_else(|| syntax(r.position(), "invalid CFF real"));
                }
                _ => return Err(syntax(r.position(), "reserved CFF real nibble")),
            };
            text.get_mut(len..len + chars.len())
                .ok_or_else(|| syntax(r.position(), "CFF real exceeds 64 characters"))?
                .copy_from_slice(chars);
            len += chars.len();
        }
    }
}

#[cfg(test)]
mod tests;
