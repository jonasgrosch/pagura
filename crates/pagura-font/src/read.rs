//! Checked, big-endian font-buffer access.
use pagura_core::{Error, Limits, Position, Result};

pub(crate) fn bad(at: usize, message: &'static str) -> Error {
    Error::syntax(message).at(Position::Buffer(at))
}

pub(crate) fn limit(at: usize, message: &'static str) -> Error {
    Error::limit(message).at(Position::Buffer(at))
}

pub(crate) fn cap(count: usize, ceiling: usize, at: usize) -> Result<()> {
    if count > ceiling {
        Err(limit(at, "font resource budget exceeded"))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Read<'a> {
    pub data: &'a [u8],
    pub pos: usize,
    pub base: usize,
}
impl<'a> Read<'a> {
    pub(crate) fn new(data: &'a [u8], base: usize) -> Self {
        Self { data, pos: 0, base }
    }
    pub(crate) fn at(self) -> usize {
        self.base.saturating_add(self.pos)
    }
    pub(crate) fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| limit(self.at(), "font offset overflow"))?;
        let bytes = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| bad(self.at(), "truncated font data"))?;
        self.pos = end;
        Ok(bytes)
    }
    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub(crate) fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    pub(crate) fn i16(&mut self) -> Result<i16> {
        let b = self.take(2)?;
        Ok(i16::from_be_bytes([b[0], b[1]]))
    }
    pub(crate) fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub(crate) fn i32(&mut self) -> Result<i32> {
        let b = self.take(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub(crate) fn offset(&mut self) -> Result<usize> {
        usize::try_from(self.u32()?).map_err(|_| limit(self.at(), "font offset overflow"))
    }
    pub(crate) fn skip(&mut self, len: usize) -> Result<()> {
        self.take(len)?;
        Ok(())
    }
    pub(crate) fn tail(&self, at: usize) -> Result<Self> {
        Ok(Self::new(
            self.data
                .get(at..)
                .ok_or_else(|| bad(self.base, "font offset outside buffer"))?,
            self.base.saturating_add(at),
        ))
    }
}

pub(crate) fn input(data: &[u8], limits: &Limits) -> Result<()> {
    cap(data.len(), limits.file_bytes.min(limits.decoded_bytes), 0)
}

pub(crate) fn finite_path(path: &pagura_raster::Path) -> Result<()> {
    use pagura_raster::Segment;
    for segment in path.segments() {
        let finite = match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => p.is_finite(),
            Segment::CubicTo(a, b, c) => a.is_finite() && b.is_finite() && c.is_finite(),
            Segment::Close => true,
        };
        if !finite {
            return Err(limit(0, "nonfinite glyph outline"));
        }
    }
    Ok(())
}
