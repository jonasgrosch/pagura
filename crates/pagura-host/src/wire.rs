//! Fallible, capped little-endian payload construction.
use pagura::{Error, Result};
pub(crate) struct Writer {
    bytes: Vec<u8>,
    cap: usize,
}
impl Writer {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            bytes: Vec::new(),
            cap: cap.min(32 * 1024 * 1024),
        }
    }
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<()> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|n| n > self.cap)
        {
            return Err(Error::limit("reading response byte budget exceeded"));
        }
        self.bytes
            .try_reserve(bytes.len())
            .map_err(|_| Error::limit("reading response allocation failed"))?;
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    pub(crate) fn u8(&mut self, value: u8) -> Result<()> {
        self.push(&[value])
    }
    pub(crate) fn u32(&mut self, value: u32) -> Result<()> {
        self.push(&value.to_le_bytes())
    }
    pub(crate) fn u64(&mut self, value: u64) -> Result<()> {
        self.push(&value.to_le_bytes())
    }
    pub(crate) fn count(&mut self, value: usize) -> Result<()> {
        self.u32(u32::try_from(value).map_err(|_| Error::limit("reading count exceeds u32"))?)
    }
    pub(crate) fn text(&mut self, value: &str) -> Result<()> {
        self.count(value.len())?;
        self.push(value.as_bytes())
    }
    pub(crate) fn f64(&mut self, value: f64) -> Result<()> {
        if !value.is_finite() {
            return Err(Error::invalid_argument("nonfinite reading coordinate"));
        }
        self.push(&value.to_le_bytes())
    }
    pub(crate) fn quad(&mut self, values: &[f64]) -> Result<()> {
        for value in values {
            self.f64(*value)?;
        }
        Ok(())
    }
    pub(crate) fn cursor(&mut self, page: u32, offset: u32, done: bool, count: u32) -> Result<()> {
        let mut header = Vec::with_capacity(13);
        header.extend(page.to_le_bytes());
        header.extend(offset.to_le_bytes());
        header.push(u8::from(done));
        header.extend(count.to_le_bytes());
        self.bytes
            .get_mut(..13)
            .ok_or(Error::invalid_argument("missing search header"))?
            .copy_from_slice(&header);
        Ok(())
    }
    pub(crate) fn finish(self) -> Vec<u8> {
        self.bytes
    }
}
