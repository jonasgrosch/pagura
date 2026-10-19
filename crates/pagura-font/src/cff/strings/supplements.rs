//! Supplemental mappings are additional codes for main-encoded glyphs
//! (TN 5176 section 12, Tables 14/15). Invalid records never replace mappings.
use crate::budget::Work;
use crate::cff::index::Index;
use crate::repairs;
use pagura_core::bytes::Cursor;
use pagura_core::{Name, Position, Result};

/// The bytes of one supplement record: a code and an SID (Table 15).
const RECORD: usize = 3;

pub(super) fn read(
    r: &mut Cursor<'_>,
    encoding: &mut [Option<Name>; 256],
    encoded: &[u16],
    strings: &Index,
    data: &[u8],
    work: &mut Work<'_>,
) -> Result<()> {
    if r.at_end() {
        return report(work, r.position());
    }
    let count = r.u8()?;
    for _ in 0..count {
        let at = r.position();
        if r.remaining() < RECORD {
            report(work, at)?;
            break;
        }
        let slot = &mut encoding[usize::from(r.u8()?)];
        let sid = r.u16()?;
        if !encoded.contains(&sid) || slot.is_some() {
            report(work, at)?;
            continue;
        }
        *slot = Some(super::sid(sid, strings, data, work, at)?);
    }
    Ok(())
}

fn report(work: &mut Work<'_>, at: Position) -> Result<()> {
    work.repair(
        repairs::CFF_ENCODING_SUPPLEMENT,
        "invalid CFF encoding supplement dropped",
        at,
    )
}
