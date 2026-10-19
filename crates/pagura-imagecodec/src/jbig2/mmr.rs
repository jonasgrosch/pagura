//! MMR-coded generic regions (T.88 6.2.6): T.6 two-dimensional rows over
//! the decoder shared with CCITT fax, with T.88's end-of-block rules.
use super::{Bitmap, bitmap::Fill};
use crate::budget::Budget;
use crate::t4::{self, Bits, Color, EOL, EOL_BITS, Extensions};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Limits, Result};

/// Whether the data must end with an EOFB code (T.88 6.2.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Eofb {
    /// EOFB may be omitted when the data ends with the last row.
    Optional,
    /// EOFB must follow the last row, as in a gray-scale bitplane.
    Required,
}

/// Decodes a `size` MMR bitmap at the start of `bytes` and moves past the
/// whole bytes it occupies. Every bit read is charged as work.
pub(super) fn decode(
    bytes: &mut Cursor<'_>,
    size: (u32, u32),
    eofb: Eofb,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Bitmap> {
    let (width, height) = size;
    let mut bitmap = Bitmap::new(width, height, Fill::White, limits, budget)?;
    let columns = u32_to_usize(width);
    // Two changing-element lists next to the bitmap keep each row's work
    // linear in its changes.
    let capacity = t4::max_changes(columns).saturating_add(1);
    let (mut reference, mut changes) = (Vec::new(), Vec::new());
    budget.reserve(&mut reference, capacity)?;
    budget.reserve(&mut changes, capacity)?;
    let mut bits = Bits::new(
        bytes.clone().with_message("truncated JBIG2 MMR data"),
        budget.work_meter(),
    );
    let stride = bitmap.stride();
    for row in bitmap.data.chunks_exact_mut(stride.max(1)) {
        changes.clear();
        t4::row_2d(
            &mut bits,
            columns,
            &reference,
            &mut changes,
            Extensions::Forbidden,
        )?;
        t4::pack_row(row, &changes, columns, Color::Black);
        std::mem::swap(&mut reference, &mut changes);
    }
    if eofb == Eofb::Required || bits.remaining() > 7 {
        for _ in 0..2 {
            let at = bits.position();
            if bits.bits(EOL_BITS)? != EOL {
                return Err(Error::syntax("invalid JBIG2 MMR EOFB").at(at));
            }
        }
    }
    bits.align()?;
    bytes.seek(bits.offset()?)?;
    budget.release::<usize>(2 * capacity);
    Ok(bitmap)
}

#[cfg(test)]
mod tests {
    use super::{Eofb, decode};
    use crate::budget::Budget;
    use crate::ccitt::{self, CcittParams, Encoding};
    use pagura_core::Limits;
    use pagura_core::bytes::Cursor;
    use pagura_testkit::fixture;
    use std::num::NonZeroU32;

    #[test]
    fn mmr_regions_decode_like_group_4_fax_with_black_as_one() {
        // T.88 6.2.6: an MMR region is T.6 data; a Group 4 fixture decodes the
        // same through both entry points (JBIG2 stores black as 1).
        let data = fixture("image", "fax-complex--1.in");
        let (width, height) = (73, 17);
        let params = CcittParams {
            encoding: Encoding::Group4,
            columns: NonZeroU32::new(width).unwrap(),
            black_is_1: true,
            ..CcittParams::default()
        };
        let fax = ccitt::decode(&data, &params, &Limits::default()).unwrap();
        let mut bytes = Cursor::new(&data);
        let limits = Limits::default();
        let mut budget = Budget::new(&limits, crate::jbig2::MESSAGES);
        let bitmap = decode(
            &mut bytes,
            (width, height),
            Eofb::Required,
            &limits,
            &mut budget,
        )
        .unwrap();
        assert_eq!(bitmap.data(), fax);
        assert!(bytes.at_end());
    }
}
