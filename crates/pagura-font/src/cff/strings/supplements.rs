//! Supplemental mappings are additional codes for main-encoded glyphs
//! (TN 5176 section 12, Tables 14/15). Invalid records never replace mappings.
use crate::budget::Work;
use crate::read::here;
use crate::repairs;
use pagura_core::bytes::Cursor;
use pagura_core::{Name, Position, Result};
use std::ops::Range;

pub(super) fn read(
    r: &mut Cursor<'_>,
    encoding: &mut [Option<Name>; 256],
    encoded: &[u16],
    strings: &[Range<usize>],
    data: &[u8],
    work: &mut Work<'_>,
) -> Result<()> {
    if r.at_end() {
        return report(work, here(r));
    }
    let count = r.u8()?;
    for _ in 0..count {
        let at = here(r);
        if r.remaining() < 3 {
            report(work, at)?;
            break;
        }
        let code = usize::from(r.u8()?);
        let sid = r.u16()?;
        if !encoded.contains(&sid) || encoding[code].is_some() {
            report(work, at)?;
            continue;
        }
        encoding[code] = Some(super::sid(sid, strings, data, work)?);
    }
    Ok(())
}
fn report(work: &mut Work<'_>, at: usize) -> Result<()> {
    work.repair(
        repairs::CFF_ENCODING_SUPPLEMENT,
        "invalid CFF encoding supplement dropped",
        Position::Buffer(at),
    )
}
