//! Operand types and dictionary domains from TN 5176 Tables 9, 10 and 23.
use super::{Dict, integer};
use crate::read::bad;
use pagura_core::{Diagnostics, Position, Result};

impl Dict {
    pub(super) fn validate(
        &mut self,
        private: bool,
        strings: usize,
        diagnostics: &mut Diagnostics,
    ) -> Result<()> {
        let mut ignored = Vec::new();
        for (&op, entry) in &self.entries {
            let values = &entry.values;
            let at = entry.at;
            let count = values.len();
            let valid = if private {
                match op {
                    6..=9 | 0x0c0c | 0x0c0d => count > 0,
                    10 | 11 | 19..=21 | 0x0c09..=0x0c0b | 0x0c0e | 0x0c11..=0x0c13 => count == 1,
                    _ => false,
                }
            } else {
                match op {
                    0..=4
                    | 13
                    | 15..=17
                    | 0x0c00..=0x0c06
                    | 0x0c08
                    | 0x0c14..=0x0c16
                    | 0x0c1f..=0x0c26 => count == 1,
                    5 => count == 4,
                    14 | 0x0c17 => count > 0,
                    18 => count == 2,
                    0x0c07 => count == 6,
                    0x0c1e => count == 3,
                    _ => false,
                }
            };
            if !valid {
                if private && matches!(op, 6..=11 | 0x0c09..=0x0c0e | 0x0c11..=0x0c12) {
                    crate::recovery::repair(
                        diagnostics,
                        crate::tags::CFF_DICT_OPERANDS,
                        "unused CFF hint operator with invalid operands ignored",
                        Position::Buffer(at),
                    )?;
                    ignored.push(op);
                    continue;
                }
                return Err(bad(at, "invalid CFF DICT operator or operands"));
            }
            if matches!(op, 0x0c01 | 0x0c0e) && !matches!(values.as_slice(), [0.0 | 1.0]) {
                return Err(bad(at, "invalid CFF boolean"));
            }
            if !private && matches!(op, 0..=4 | 0x0c00 | 0x0c15 | 0x0c16 | 0x0c26 | 0x0c1e) {
                let sids = if op == 0x0c1e { &values[..2] } else { values };
                for &sid in sids {
                    let sid = integer(sid, at)?;
                    if sid >= 65000 || sid >= 391 + strings {
                        return Err(bad(at, "CFF DICT SID outside String INDEX"));
                    }
                }
            }
        }
        for op in ignored {
            self.entries.remove(&op);
        }
        Ok(())
    }
}
