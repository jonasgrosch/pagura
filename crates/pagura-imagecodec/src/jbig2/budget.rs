//! Cumulative work accounting, shared by all segments and coding procedures.
use pagura_core::{Error, Limits, Result};

#[derive(Default)]
pub(super) struct Work {
    pixels: usize,
    operations: usize,
}

impl Work {
    pub(super) fn pixels(&mut self, pixels: u64, limits: &Limits) -> Result<()> {
        // One decoded/composited pixel plus at most sixteen template samples.
        let cost = pixels
            .checked_mul(18)
            .and_then(|n| usize::try_from(n).ok())
            .filter(|&n| n <= limits.raster_work.saturating_sub(self.pixels))
            .ok_or_else(|| Error::limit("JBIG2 cumulative pixel work exceeds budget"))?;
        self.pixels += cost;
        Ok(())
    }

    pub(super) fn operations(&mut self, count: usize, limits: &Limits) -> Result<()> {
        if count > limits.operations.saturating_sub(self.operations) {
            return Err(Error::limit("JBIG2 cumulative operations exceed budget"));
        }
        self.operations += count;
        Ok(())
    }
}
