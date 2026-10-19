//! CFF INDEX data (TN 5176 section 5; CFF2 INDEXes have 32-bit counts).
use super::Version;
use crate::budget::Work;
use crate::memory::vector;
use crate::read::{self, here, limit, syntax};
use pagura_core::Result;
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use std::ops::{Range, RangeInclusive};

/// The valid widths of an INDEX offset field (TN 5176 Table 2, OffSize).
pub(super) const OFFSET_SIZES: RangeInclusive<u8> = 1..=4;

/// A validated INDEX: object `i` occupies `offsets[i]..offsets[i + 1]`,
/// absolute offsets in the program that lie inside it.
#[derive(Debug, Default)]
pub(super) struct Index {
    offsets: Vec<usize>,
}

impl Index {
    /// Reads an INDEX at the cursor and moves past its data.
    pub(super) fn parse(r: &mut Cursor<'_>, version: Version, work: &mut Work<'_>) -> Result<Self> {
        let at = r.position();
        let count = match version {
            Version::Cff1 => usize::from(r.u16()?),
            Version::Cff2 => u32_to_usize(r.u32()?),
        };
        work.entries(count, at)?;
        if count == 0 {
            return Ok(Self::default());
        }
        let size_at = r.position();
        let size = r.u8()?;
        if !OFFSET_SIZES.contains(&size) {
            return Err(syntax(size_at, "invalid CFF offset size"));
        }
        let mut relative = Vec::new();
        relative
            .try_reserve_exact(count + 1)
            .map_err(|_| limit(at, "CFF INDEX allocation failed"))?;
        // Offsets count from the byte before the data, so the first is 1.
        let mut previous = 1;
        for i in 0..=count {
            let offset_at = r.position();
            let offset = usize::try_from(r.uint(usize::from(size))?)
                .map_err(|_| limit(offset_at, "CFF offset overflow"))?;
            if offset < previous || (i == 0 && offset != 1) {
                return Err(syntax(r.position(), "invalid CFF INDEX offsets"));
            }
            relative.push(offset - 1);
            previous = offset;
        }
        let start = here(r);
        r.skip(previous - 1)?;
        for offset in &mut relative {
            *offset += start;
        }
        Ok(Self { offsets: relative })
    }

    pub(super) fn len(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.offsets.len() < 2
    }

    /// The program bytes of object `i`.
    pub(super) fn range(&self, i: usize) -> Option<Range<usize>> {
        Some(*self.offsets.get(i)?..*self.offsets.get(i.checked_add(1)?)?)
    }

    /// A cursor over object `i` of `data`, the program this INDEX was read from.
    pub(super) fn get<'a>(&self, data: &'a [u8], i: usize) -> Option<Cursor<'a>> {
        read::slice(data, self.range(i)?, "CFF INDEX object outside font").ok()
    }

    /// The objects, in order.
    pub(super) fn ranges(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        self.offsets
            .iter()
            .zip(self.offsets.iter().skip(1))
            .map(|(&start, &end)| start..end)
    }

    pub(super) fn heap_bytes(&self) -> usize {
        vector(&self.offsets)
    }
}

#[cfg(test)]
mod tests {
    use super::{Index, Version};
    use crate::budget::{FontBudget, Work};
    use crate::read::cursor;
    use pagura_core::{Diagnostics, ErrorKind, Limits, Recovery};

    fn parse(bytes: &[u8], version: Version, limits: &Limits) -> pagura_core::Result<Index> {
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(limits), &mut diagnostics);
        let mut r = cursor(bytes).with_base(100);
        let index = Index::parse(&mut r, version, &mut work)?;
        assert!(r.at_end());
        Ok(index)
    }

    #[test]
    fn objects_are_absolute_checked_ranges() {
        // Two objects of 2 and 1 bytes, offset size 1.
        let data = [0, 2, 1, 1, 3, 4, b'a', b'b', b'c'];
        let index = parse(&data, Version::Cff1, &Limits::default()).unwrap();
        assert_eq!(index.len(), 2);
        assert_eq!(index.range(0), Some(106..108));
        assert_eq!(index.range(1), Some(108..109));
        assert_eq!(index.range(2), None);
        assert_eq!(index.ranges().collect::<Vec<_>>(), [106..108, 108..109]);
        let cff2 = [0, 0, 0, 1, 2, 0, 1, 0, 2, b'x'];
        assert_eq!(
            parse(&cff2, Version::Cff2, &Limits::default())
                .unwrap()
                .range(0),
            Some(109..110)
        );
        assert!(
            parse(&[0, 0], Version::Cff1, &Limits::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn malformed_offsets_are_syntax_and_counts_are_budgeted() {
        for malformed in [
            &[0, 1, 5, 1, 2][..],    // offset size 5
            &[0, 1, 1, 2, 2, 0][..], // first offset is not 1
            &[0, 1, 1, 1, 3, 0][..], // data truncated
        ] {
            let error = parse(malformed, Version::Cff1, &Limits::default()).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Syntax);
        }
        let small = Limits {
            font_entries: 1,
            ..Limits::default()
        };
        let data = [0, 2, 1, 1, 2, 3, 0, 0];
        assert_eq!(
            parse(&data, Version::Cff1, &small).unwrap_err().kind,
            ErrorKind::Limit
        );
    }
}
