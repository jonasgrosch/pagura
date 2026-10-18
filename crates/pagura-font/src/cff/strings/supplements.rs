//! Supplemental mappings are additional codes for main-encoded glyphs
//! (TN 5176 section 12, Tables 14/15). Invalid records never replace mappings.
use crate::read::Read;
use crate::{recovery::repair, tags};
use pagura_core::{Diagnostics, Limits, Name, Position, Result};
use std::ops::Range;

pub(super) fn read(
    r: &mut Read<'_>,
    encoding: &mut [Option<Name>; 256],
    encoded: &[u16],
    strings: &[Range<usize>],
    data: &[u8],
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    if r.pos == r.data.len() {
        return report(diagnostics, r.at());
    }
    let count = r.u8()?;
    for _ in 0..count {
        let at = r.at();
        if r.data.len() - r.pos < 3 {
            report(diagnostics, at)?;
            break;
        }
        let code = usize::from(r.u8()?);
        let sid = r.u16()?;
        if !encoded.contains(&sid) || encoding[code].is_some() {
            report(diagnostics, at)?;
            continue;
        }
        encoding[code] = Some(super::sid(sid, strings, data, limits)?);
    }
    Ok(())
}
fn report(diagnostics: &mut Diagnostics, at: usize) -> Result<()> {
    repair(
        diagnostics,
        tags::CFF_ENCODING_SUPPLEMENT,
        "invalid CFF encoding supplement dropped",
        Position::Buffer(at),
    )
}
