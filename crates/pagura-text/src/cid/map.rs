//! Disjoint CID intervals: logarithmic lookup without expanding 32-bit codes.
use crate::{cid::Cid, cmap::Code};
use pagura_core::{Error, Meter, Result};
use std::collections::BTreeMap;

/// How the codes of a range map to CIDs (9.7.5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// `cidrange`: consecutive codes map to consecutive CIDs.
    Increment,
    /// `notdefrange`: every code maps to the same CID.
    Constant,
}

/// Codes `first..=last` of one length, mapped from `cid` on. Construction
/// guarantees that every code maps to a CID, so lookups cannot overflow.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CidRange {
    first: Code,
    last: Code,
    cid: Cid,
    step: Step,
}

impl CidRange {
    pub(crate) fn new(first: Code, last: Code, cid: Cid, step: Step) -> Result<Self> {
        if first.len != last.len || first.value > last.value {
            return Err(Error::syntax("invalid CMap range"));
        }
        if step == Step::Increment && cid.checked_add(last.value - first.value).is_none() {
            return Err(Error::syntax("CMap CID range overflows"));
        }
        Ok(Self {
            first,
            last,
            cid,
            step,
        })
    }

    fn at(self, code: Code) -> Option<Cid> {
        match self.step {
            Step::Increment => self.cid.checked_add(code.value - self.first.value),
            Step::Constant => Some(self.cid),
        }
    }

    fn last_cid(self) -> Cid {
        self.at(self.last).unwrap_or(self.cid)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CidMap(BTreeMap<Code, CidRange>);

impl CidMap {
    /// Every 2-byte code to the same CID: the Identity-H/V mapping (9.7.5.2).
    pub(crate) fn identity() -> Self {
        let first = Code { value: 0, len: 2 };
        let last = Code {
            value: u32::from(u16::MAX),
            len: 2,
        };
        Self(BTreeMap::from([(
            first,
            CidRange {
                first,
                last,
                cid: Cid::NOTDEF,
                step: Step::Increment,
            },
        )]))
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        crate::memory::tree::<Code, CidRange>(self.0.len())
    }

    pub(crate) fn cids(&self) -> impl Iterator<Item = Cid> + '_ {
        self.0
            .values()
            .flat_map(|range| (range.cid.get()..=range.last_cid().get()).map(Cid::new))
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn get(&self, code: Code, work: &mut Meter) -> Result<Option<Cid>> {
        work.charge(1)?;
        Ok(self
            .0
            .range(..=code)
            .next_back()
            .filter(|(_, range)| range.first.len == code.len && code.value <= range.last.value)
            .and_then(|(_, range)| range.at(code)))
    }

    /// New definitions override older definitions; retained fragments keep
    /// their original CID offsets, including constant notdef ranges. The
    /// caller charges each definition as one entry, which also bounds the
    /// fragments (at most one more per definition).
    pub(crate) fn insert(&mut self, range: CidRange, work: &mut Meter) -> Result<()> {
        let mut overlap = Vec::new();
        if let Some((&key, old)) = self.0.range(..range.first).next_back()
            && old.first.len == range.first.len
            && old.last.value >= range.first.value
        {
            overlap.push(key);
        }
        for (&key, _) in self.0.range(range.first..=range.last) {
            work.charge(1)?;
            overlap.push(key);
        }
        for key in overlap {
            work.charge(1)?;
            if let Some(old) = self.0.remove(&key) {
                // Both fragments are sub-ranges of a valid range, so they stay valid.
                if old.first.value < range.first.value {
                    let last = Code {
                        value: range.first.value - 1,
                        len: old.first.len,
                    };
                    self.0.insert(old.first, CidRange { last, ..old });
                }
                if old.last.value > range.last.value {
                    let first = Code {
                        value: range.last.value + 1,
                        len: old.first.len,
                    };
                    let cid = old.at(first).unwrap_or(old.cid);
                    self.0.insert(first, CidRange { first, cid, ..old });
                }
            }
        }
        work.charge(1)?;
        self.0.insert(range.first, range);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(first: u32, last: u32, cid: u16, len: u8, step: Step) -> CidRange {
        CidRange::new(
            Code { value: first, len },
            Code { value: last, len },
            Cid::new(cid),
            step,
        )
        .unwrap()
    }

    #[test]
    fn overwrites_split_preserving_offsets_and_code_lengths() -> Result<()> {
        let mut map = CidMap::default();
        let mut work = Meter::new(usize::MAX, "test");
        map.insert(range(10, 20, 100, 2, Step::Increment), &mut work)?;
        map.insert(range(12, 14, 500, 2, Step::Increment), &mut work)?;
        map.insert(range(13, 13, 900, 2, Step::Constant), &mut work)?;
        map.insert(range(0, 255, 40, 1, Step::Constant), &mut work)?;
        for (code, cid) in [
            (9, None),
            (10, Some(100)),
            (11, Some(101)),
            (12, Some(500)),
            (13, Some(900)),
            (14, Some(502)),
            (15, Some(105)),
            (20, Some(110)),
            (21, None),
        ] {
            let code = Code {
                value: code,
                len: 2,
            };
            assert_eq!(map.get(code, &mut work)?, cid.map(Cid::new));
        }
        let code = Code { value: 13, len: 1 };
        assert_eq!(map.get(code, &mut work)?, Some(Cid::new(40)));
        map.insert(range(0, 255, 0, 2, Step::Increment), &mut work)?;
        let code = Code { value: 13, len: 2 };
        assert_eq!(map.get(code, &mut work)?, Some(Cid::new(13)));
        assert_eq!(map.len(), 2);
        Ok(())
    }

    #[test]
    fn ranges_that_would_leave_sixteen_bits_are_rejected() {
        let code = |value| Code { value, len: 2 };
        assert!(CidRange::new(code(0), code(1), Cid::new(65_535), Step::Increment).is_err());
        assert!(CidRange::new(code(0), code(9), Cid::new(65_535), Step::Constant).is_ok());
        assert!(CidRange::new(code(5), code(4), Cid::new(0), Step::Increment).is_err());
    }
}
