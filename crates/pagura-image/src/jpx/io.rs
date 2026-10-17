//! Checked binary access and the shared allocation/work budget.

use pagura_core::{Error, Limits, Position, Result};

pub(super) fn syntax(message: &'static str, offset: usize) -> Error {
    Error::syntax(message).at(Position::Buffer(offset))
}

pub(super) struct Span {
    pub logical: usize,
    pub source: usize,
}

pub(super) fn source_position(spans: &[Span], logical: usize, fallback: usize) -> usize {
    let index = spans.partition_point(|s| s.logical <= logical);
    index
        .checked_sub(1)
        .and_then(|i| spans.get(i))
        .map_or(fallback, |span| span.source + (logical - span.logical))
}

pub(super) struct Reader<'a> {
    pub data: &'a [u8],
    pub offset: usize,
    pub base: usize,
    source_map: Option<&'a [Span]>,
    logical_origin: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], base: usize) -> Self {
        Self {
            data,
            offset: 0,
            base,
            source_map: None,
            logical_origin: 0,
        }
    }

    pub fn mapped(data: &'a [u8], origin: usize, spans: &'a [Span]) -> Self {
        Self {
            data,
            offset: 0,
            base: 0,
            source_map: Some(spans),
            logical_origin: origin,
        }
    }

    pub fn position(&self) -> usize {
        if let Some(spans) = self.source_map {
            let logical = self.logical_origin + self.offset;
            return source_position(spans, logical, self.base + self.offset);
        }
        self.base + self.offset
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.offset
    }

    pub fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or_else(|| syntax("JPEG 2000 offset overflow", self.position()))?;
        let bytes = self
            .data
            .get(self.offset..end)
            .ok_or_else(|| syntax("truncated JPEG 2000 data", self.position()))?;
        self.offset = end;
        Ok(bytes)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A.7.4/A.7.5 require every marker fragment to end on a header boundary.
    pub fn packet_boundary(&self, start: usize) -> Result<()> {
        if let Some(spans) = self.source_map {
            let lo = self.logical_origin + start;
            let hi = self.logical_origin + self.offset;
            let index = spans.partition_point(|s| s.logical <= lo);
            if let Some(span) = spans.get(index).filter(|s| s.logical < hi) {
                return Err(syntax(
                    "JPEG 2000 packed marker splits a packet header",
                    span.source,
                ));
            }
        }
        Ok(())
    }

    pub fn finish(&self) -> Result<()> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(syntax("extra JPEG 2000 field data", self.position()))
        }
    }
}

pub(super) struct Budget<'a> {
    pub limits: &'a Limits,
    bytes: usize,
    work: usize,
}

impl<'a> Budget<'a> {
    pub fn new(limits: &'a Limits) -> Self {
        Self {
            limits,
            bytes: 0,
            work: 0,
        }
    }

    pub fn work(&mut self, count: usize) -> Result<()> {
        self.work = self
            .work
            .checked_add(count)
            .filter(|&n| n <= self.limits.operations)
            .ok_or_else(|| Error::limit("JPEG 2000 work budget exceeded"))?;
        Ok(())
    }

    pub fn charge<T>(&mut self, count: usize) -> Result<()> {
        // Initializing or copying each allocation is work even before entropy decoding.
        self.work(count)?;
        self.bytes = count
            .checked_mul(size_of::<T>())
            .and_then(|n| self.bytes.checked_add(n))
            .filter(|&n| n <= self.limits.decoded_bytes)
            .ok_or_else(|| Error::limit("JPEG 2000 allocation budget exceeded"))?;
        Ok(())
    }

    pub fn push<T>(&mut self, values: &mut Vec<T>, value: T) -> Result<()> {
        if values.len() == values.capacity() {
            let capacity = values
                .len()
                .checked_mul(2)
                .map(|n| n.max(1))
                .ok_or_else(|| Error::limit("JPEG 2000 vector capacity overflow"))?;
            self.charge::<T>(capacity)?;
            values
                .try_reserve_exact(capacity - values.len())
                .map_err(|_| Error::limit("JPEG 2000 allocation failed"))?;
        }
        values.push(value);
        Ok(())
    }

    pub fn append(&mut self, values: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
        let needed = values
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| Error::limit("JPEG 2000 byte length overflow"))?;
        if needed > values.capacity() {
            let capacity = needed
                .checked_next_power_of_two()
                .ok_or_else(|| Error::limit("JPEG 2000 byte capacity overflow"))?;
            self.charge::<u8>(capacity)?;
            values
                .try_reserve_exact(capacity - values.len())
                .map_err(|_| Error::limit("JPEG 2000 allocation failed"))?;
        }
        self.work(bytes.len())?;
        values.extend_from_slice(bytes);
        Ok(())
    }

    pub fn filled<T: Clone>(&mut self, count: usize, value: T) -> Result<Vec<T>> {
        self.charge::<T>(count)?;
        let mut result = Vec::new();
        result
            .try_reserve_exact(count)
            .map_err(|_| Error::limit("JPEG 2000 allocation failed"))?;
        result.resize(count, value);
        Ok(result)
    }
}

pub(super) fn usize_from(value: u32) -> Result<usize> {
    usize::try_from(value).map_err(|_| Error::limit("JPEG 2000 size exceeds address space"))
}

pub(super) fn area(width: u32, height: u32) -> Result<usize> {
    usize_from(width)?
        .checked_mul(usize_from(height)?)
        .ok_or_else(|| Error::limit("JPEG 2000 sample count overflow"))
}
