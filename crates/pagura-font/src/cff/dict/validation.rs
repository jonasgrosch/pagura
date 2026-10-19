//! Operand types and dictionary domains from TN 5176 Tables 9, 10 and 23.
use super::{Dict, Operator as Op};
use crate::budget::Work;
use crate::cff::strings::{RESERVED_SID, STANDARD_STRINGS};
use crate::charstring::integer;
use crate::read::syntax;
use crate::repairs;
use pagura_core::Result;

/// The dictionary whose operator domain applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::cff) enum Domain {
    /// A Top or Font DICT, whose SIDs index a String INDEX of `strings`.
    Top { strings: usize },
    /// A Private DICT.
    Private,
}

/// How many operands an operator takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arity {
    Exactly(usize),
    /// A nonempty array or delta array.
    Array,
}

impl Arity {
    const fn admits(self, count: usize) -> bool {
        match self {
            Self::Exactly(n) => count == n,
            Self::Array => count > 0,
        }
    }
}

fn top_arity(op: Op) -> Option<Arity> {
    Some(match op {
        Op::VERSION
        | Op::NOTICE
        | Op::FULL_NAME
        | Op::FAMILY_NAME
        | Op::WEIGHT
        | Op::UNIQUE_ID
        | Op::CHARSET
        | Op::ENCODING
        | Op::CHAR_STRINGS
        | Op::COPYRIGHT
        | Op::IS_FIXED_PITCH
        | Op::ITALIC_ANGLE
        | Op::UNDERLINE_POSITION
        | Op::UNDERLINE_THICKNESS
        | Op::PAINT_TYPE
        | Op::CHARSTRING_TYPE
        | Op::STROKE_WIDTH
        | Op::SYNTHETIC_BASE
        | Op::POST_SCRIPT
        | Op::BASE_FONT_NAME
        | Op::CID_FONT_VERSION
        | Op::CID_FONT_REVISION
        | Op::CID_FONT_TYPE
        | Op::CID_COUNT
        | Op::UID_BASE
        | Op::FD_ARRAY
        | Op::FD_SELECT
        | Op::FONT_NAME => Arity::Exactly(1),
        Op::PRIVATE => Arity::Exactly(2),
        Op::ROS => Arity::Exactly(3),
        Op::FONT_BBOX => Arity::Exactly(4),
        Op::FONT_MATRIX => Arity::Exactly(6),
        Op::XUID | Op::BASE_FONT_BLEND => Arity::Array,
        _ => return None,
    })
}

fn private_arity(op: Op) -> Option<Arity> {
    Some(match op {
        Op::BLUE_VALUES
        | Op::OTHER_BLUES
        | Op::FAMILY_BLUES
        | Op::FAMILY_OTHER_BLUES
        | Op::STEM_SNAP_H
        | Op::STEM_SNAP_V => Arity::Array,
        Op::STD_HW
        | Op::STD_VW
        | Op::SUBRS
        | Op::DEFAULT_WIDTH_X
        | Op::NOMINAL_WIDTH_X
        | Op::BLUE_SCALE
        | Op::BLUE_SHIFT
        | Op::BLUE_FUZZ
        | Op::FORCE_BOLD
        | Op::LANGUAGE_GROUP
        | Op::EXPANSION_FACTOR
        | Op::INITIAL_RANDOM_SEED => Arity::Exactly(1),
        _ => return None,
    })
}

/// Private DICT hint values an unhinted reader never uses, so invalid
/// operands are a reported repair rather than an error.
const fn unused_hint(op: Op) -> bool {
    matches!(
        op,
        Op::BLUE_VALUES
            | Op::OTHER_BLUES
            | Op::FAMILY_BLUES
            | Op::FAMILY_OTHER_BLUES
            | Op::STD_HW
            | Op::STD_VW
            | Op::BLUE_SCALE
            | Op::BLUE_SHIFT
            | Op::BLUE_FUZZ
            | Op::STEM_SNAP_H
            | Op::STEM_SNAP_V
            | Op::FORCE_BOLD
            | Op::LANGUAGE_GROUP
            | Op::EXPANSION_FACTOR
    )
}

impl Dict {
    pub(in crate::cff) fn validate(&mut self, domain: Domain, work: &mut Work<'_>) -> Result<()> {
        let mut ignored = Vec::new();
        for (&op, entry) in &self.entries {
            let values = entry.values.as_slice();
            let at = entry.at;
            let arity = match domain {
                Domain::Top { .. } => top_arity(op),
                Domain::Private => private_arity(op),
            };
            if !arity.is_some_and(|arity| arity.admits(values.len())) {
                if domain == Domain::Private && unused_hint(op) {
                    work.repair(
                        repairs::CFF_DICT_OPERANDS,
                        "unused CFF hint operator with invalid operands ignored",
                        at,
                    )?;
                    ignored.push(op);
                    continue;
                }
                return Err(syntax(at, "invalid CFF DICT operator or operands"));
            }
            if matches!(op, Op::IS_FIXED_PITCH | Op::FORCE_BOLD) && !matches!(values, [0.0 | 1.0]) {
                return Err(syntax(at, "invalid CFF boolean"));
            }
            let Domain::Top { strings } = domain else {
                continue;
            };
            let sids: &[f64] = match (op, values) {
                // Registry and Ordering are SIDs; Supplement is a number.
                (Op::ROS, [names @ .., _supplement]) => names,
                (
                    Op::VERSION
                    | Op::NOTICE
                    | Op::FULL_NAME
                    | Op::FAMILY_NAME
                    | Op::WEIGHT
                    | Op::COPYRIGHT
                    | Op::POST_SCRIPT
                    | Op::BASE_FONT_NAME
                    | Op::FONT_NAME,
                    sid @ [_],
                ) => sid,
                _ => &[],
            };
            for &sid in sids {
                let sid = integer(sid, at)?;
                if sid >= usize::from(RESERVED_SID) || sid >= STANDARD_STRINGS + strings {
                    return Err(syntax(at, "CFF DICT SID outside String INDEX"));
                }
            }
        }
        for op in ignored {
            self.entries.remove(&op);
        }
        Ok(())
    }
}
