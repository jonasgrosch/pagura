//! Character identifiers (9.7.2) and the CIDToGIDMap stream (9.7.4.2).
use pagura_core::{Error, Result};

/// A CID. PDF character identifiers are 0 to 65,535 (9.7.2; Annex C), so the
/// range invariant cannot be violated once a value exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub(crate) struct Cid(u16);

impl Cid {
    /// CID 0, the `.notdef` glyph of every CIDFont (9.7.6.3).
    pub(crate) const NOTDEF: Self = Self(0);

    pub(crate) const fn new(value: u16) -> Self {
        Self(value)
    }

    pub(crate) const fn get(self) -> u16 {
        self.0
    }

    /// The CID `offset` places after this one, if it is still a CID.
    pub(crate) fn checked_add(self, offset: u32) -> Option<Self> {
        u32::from(self.0)
            .checked_add(offset)
            .and_then(|value| u16::try_from(value).ok())
            .map(Self)
    }

    /// Reads a PDF integer as a CID.
    pub(crate) fn from_integer(value: i64) -> Option<Self> {
        u16::try_from(value).ok().map(Self)
    }
}

impl From<Cid> for u32 {
    fn from(cid: Cid) -> Self {
        u32::from(cid.0)
    }
}

impl TryFrom<u32> for Cid {
    type Error = Error;

    fn try_from(value: u32) -> Result<Self> {
        u16::try_from(value)
            .map(Self)
            .map_err(|_| Error::syntax("CID outside 16-bit range"))
    }
}

/// An embedded CIDFontType2 `CIDToGIDMap` stream: the glyph index of CID `n`
/// is the big-endian 16-bit value at bytes `2n` and `2n + 1` (Table 115).
#[derive(Debug)]
pub(crate) struct CidToGidMap(Box<[u8]>);

impl CidToGidMap {
    /// The longest meaningful stream: two bytes for each of the 65,536 CIDs.
    pub(crate) const MAX_BYTES: usize = 2 * (1 << 16);

    /// Validates the decoded stream length.
    pub(crate) fn new(bytes: Box<[u8]>) -> Result<Self> {
        if !bytes.len().is_multiple_of(2) || bytes.len() > Self::MAX_BYTES {
            return Err(Error::syntax("invalid CIDToGIDMap length"));
        }
        Ok(Self(bytes))
    }

    /// The glyph index the stream assigns to `cid`, or `None` past its end.
    pub(crate) fn get(&self, cid: Cid) -> Option<u16> {
        let offset = 2 * usize::from(cid.get());
        let bytes = self.0.get(offset..offset + 2)?;
        Some(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.0.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cids_stay_in_sixteen_bits() {
        assert_eq!(Cid::new(65_534).checked_add(1), Some(Cid::new(65_535)));
        assert_eq!(Cid::new(65_535).checked_add(1), None);
        assert_eq!(Cid::from_integer(65_536), None);
        assert_eq!(Cid::from_integer(-1), None);
        assert!(Cid::try_from(70_000).is_err());
    }

    #[test]
    fn cid_to_gid_streams_are_even_and_bounded() -> Result<()> {
        let map = CidToGidMap::new([0, 7, 1, 2].into())?;
        assert_eq!(map.get(Cid::new(0)), Some(7));
        assert_eq!(map.get(Cid::new(1)), Some(258));
        assert_eq!(map.get(Cid::new(2)), None);
        assert!(CidToGidMap::new([0].into()).is_err());
        assert!(CidToGidMap::new(vec![0; CidToGidMap::MAX_BYTES + 2].into()).is_err());
        assert!(CidToGidMap::new(vec![0; CidToGidMap::MAX_BYTES].into()).is_ok());
        Ok(())
    }
}
