//! The one bounded little-endian writer behind every response.

use crate::protocol::PositionKind;
use pagura::{Error, ObjectId, Position, Result};

/// Appends little-endian fields, refusing to grow past its cap and
/// reserving memory fallibly before each append.
#[derive(Debug)]
pub(crate) struct Writer {
    bytes: Vec<u8>,
    cap: usize,
}

/// `N` bytes reserved by [`Writer::slot`], filled once their value is known.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Slot<const N: usize>(usize);

impl Writer {
    pub(crate) const fn new(cap: usize) -> Self {
        Self {
            bytes: Vec::new(),
            cap,
        }
    }

    pub(crate) const fn len(&self) -> usize {
        self.bytes.len()
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<()> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|size| size > self.cap)
        {
            return Err(Error::limit("response frame budget exceeded"));
        }
        self.bytes
            .try_reserve(bytes.len())
            .map_err(|_| Error::limit("response allocation failed"))?;
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    pub(crate) fn u8(&mut self, value: u8) -> Result<()> {
        self.push(&[value])
    }

    pub(crate) fn flag(&mut self, value: bool) -> Result<()> {
        self.u8(u8::from(value))
    }

    pub(crate) fn u16(&mut self, value: u16) -> Result<()> {
        self.push(&value.to_le_bytes())
    }

    pub(crate) fn u32(&mut self, value: u32) -> Result<()> {
        self.push(&value.to_le_bytes())
    }

    pub(crate) fn u64(&mut self, value: u64) -> Result<()> {
        self.push(&value.to_le_bytes())
    }

    /// A count or offset, which the wire carries as `u32`.
    pub(crate) fn count(&mut self, count: usize) -> Result<()> {
        self.u32(u32::try_from(count).map_err(|_| Error::limit("response count exceeds u32"))?)
    }

    /// Geometry, which is always finite on the wire.
    pub(crate) fn f64(&mut self, value: f64) -> Result<()> {
        if !value.is_finite() {
            return Err(Error::invalid_argument("nonfinite response coordinate"));
        }
        self.push(&value.to_le_bytes())
    }

    pub(crate) fn f64s(&mut self, values: &[f64]) -> Result<()> {
        values.iter().try_for_each(|&value| self.f64(value))
    }

    /// `u32 length | UTF-8 bytes`.
    pub(crate) fn text(&mut self, text: &str) -> Result<()> {
        self.count(text.len())?;
        self.push(text.as_bytes())
    }

    /// `u32 number | u16 generation`.
    pub(crate) fn object(&mut self, object: ObjectId) -> Result<()> {
        self.u32(object.number)?;
        self.u16(object.generation)
    }

    /// `u8 has_object [object] | u8 position_kind [u64 offset [object]]`.
    pub(crate) fn context(&mut self, object: Option<ObjectId>, position: Position) -> Result<()> {
        self.flag(object.is_some())?;
        if let Some(object) = object {
            self.object(object)?;
        }
        let (kind, offset, stream) = match position {
            Position::File(offset) => (PositionKind::File, Some(offset), None),
            Position::Stream { object, offset } => {
                (PositionKind::Stream, Some(offset), Some(object))
            }
            Position::Buffer(offset) => (PositionKind::Buffer, Some(offset), None),
            // Unknown, or a position kind added to pagura-core that has no
            // code until a protocol version gives it one.
            _ => (PositionKind::Unknown, None, None),
        };
        self.u8(kind.code())?;
        if let Some(offset) = offset {
            self.u64(
                u64::try_from(offset).map_err(|_| Error::limit("position offset overflows"))?,
            )?;
        }
        if let Some(object) = stream {
            self.object(object)?;
        }
        Ok(())
    }

    /// Reserves `N` zero bytes for a field written later with [`Self::fill`].
    pub(crate) fn slot<const N: usize>(&mut self) -> Result<Slot<N>> {
        let at = self.bytes.len();
        self.push(&[0; N])?;
        Ok(Slot(at))
    }

    pub(crate) fn fill<const N: usize>(&mut self, slot: Slot<N>, bytes: [u8; N]) {
        let span = self
            .bytes
            .get_mut(slot.0..)
            .and_then(<[u8]>::first_chunk_mut::<N>);
        debug_assert!(span.is_some(), "slot from another writer");
        if let Some(span) = span {
            *span = bytes;
        }
    }

    pub(crate) fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_counts_and_slots_are_exact() {
        let mut out = Writer::new(9);
        let slot = out.slot::<4>().unwrap();
        out.u8(7).unwrap();
        out.u32(0x0403_0201).unwrap();
        assert_eq!(out.u8(0).unwrap_err().kind, pagura::ErrorKind::Limit);
        out.fill(slot, [9, 8, 7, 6]);
        assert_eq!(out.finish(), [9, 8, 7, 6, 7, 1, 2, 3, 4]);
        let mut out = Writer::new(64);
        assert_eq!(
            out.f64(f64::NAN).unwrap_err().kind,
            pagura::ErrorKind::InvalidArgument
        );
        out.context(None, Position::Unknown).unwrap();
        assert_eq!(out.finish(), [0, 0]);
    }
}
