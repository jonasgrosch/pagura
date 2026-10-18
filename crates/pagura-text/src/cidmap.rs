//! Disjoint CID intervals: logarithmic lookup without expanding 32-bit codes.
use crate::cmap::{Code, charge};
use pagura_core::{Error, Limits, Result};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CidRange {
    pub(crate) first: Code,
    pub(crate) last: Code,
    pub(crate) cid: u32,
    pub(crate) increment: bool,
}

impl CidRange {
    fn at(self, code: Code) -> u32 {
        self.cid
            + if self.increment {
                code.value - self.first.value
            } else {
                0
            }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CidMap(BTreeMap<Code, CidRange>);

impl CidMap {
    pub(crate) fn heap_bytes(&self) -> usize {
        crate::memory::tree::<Code, CidRange>(self.0.len())
    }

    pub(crate) fn cids(&self) -> impl Iterator<Item = u32> + '_ {
        self.0.values().flat_map(|range| {
            let end = if range.increment {
                range.cid + (range.last.value - range.first.value)
            } else {
                range.cid
            };
            range.cid..=end
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn get(&self, code: Code, work: &mut usize, limits: Limits) -> Result<Option<u32>> {
        charge(work, 1, limits.operations)?;
        Ok(self.0.range(..=code).next_back().and_then(|(_, range)| {
            (range.first.len == code.len && code.value <= range.last.value).then(|| range.at(code))
        }))
    }

    /// New definitions override older definitions; retained fragments keep
    /// their original CID offsets, including constant notdef ranges.
    pub(crate) fn insert(
        &mut self,
        range: CidRange,
        work: &mut usize,
        limits: Limits,
    ) -> Result<()> {
        let mut overlap = Vec::new();
        if let Some((&key, old)) = self.0.range(..range.first).next_back()
            && old.first.len == range.first.len
            && old.last.value >= range.first.value
        {
            overlap.push(key);
        }
        for (&key, _) in self.0.range(range.first..=range.last) {
            charge(work, 1, limits.operations)?;
            overlap.push(key);
        }
        for key in overlap {
            charge(work, 1, limits.operations)?;
            if let Some(old) = self.0.remove(&key) {
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
                    self.0.insert(
                        first,
                        CidRange {
                            first,
                            cid: old.at(first),
                            ..old
                        },
                    );
                }
            }
        }
        if self.0.len() >= limits.value_nodes {
            return Err(Error::limit("CMap interval budget exceeded"));
        }
        charge(work, 1, limits.operations)?;
        self.0.insert(range.first, range);
        Ok(())
    }

    pub(crate) fn overlay(&mut self, child: Self, work: &mut usize, limits: Limits) -> Result<()> {
        for range in child.0.into_values() {
            self.insert(range, work, limits)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn range(first: u32, last: u32, cid: u32, len: u8, increment: bool) -> CidRange {
        CidRange {
            first: Code { value: first, len },
            last: Code { value: last, len },
            cid,
            increment,
        }
    }
    #[test]
    fn overwrites_split_preserving_offsets_and_code_lengths() -> Result<()> {
        let limits = Limits::default();
        let mut map = CidMap::default();
        let mut work = 0;
        map.insert(range(10, 20, 100, 2, true), &mut work, limits)?;
        map.insert(range(12, 14, 500, 2, true), &mut work, limits)?;
        map.insert(range(13, 13, 900, 2, false), &mut work, limits)?;
        map.insert(range(0, 255, 40, 1, false), &mut work, limits)?;
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
            assert_eq!(
                map.get(
                    Code {
                        value: code,
                        len: 2
                    },
                    &mut work,
                    limits
                )?,
                cid
            );
        }
        assert_eq!(
            map.get(Code { value: 13, len: 1 }, &mut work, limits)?,
            Some(40)
        );
        map.insert(range(0, 255, 0, 2, true), &mut work, limits)?;
        assert_eq!(
            map.get(Code { value: 13, len: 2 }, &mut work, limits)?,
            Some(13)
        );
        assert_eq!(map.len(), 2);
        Ok(())
    }
}
