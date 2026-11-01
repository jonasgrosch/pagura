//! Cumulative work accounting, shared by all segments and coding procedures.
use pagura_core::{Error, Limits, Result};

/// Work charged against `decode_work`: decoded and composited pixels, and
/// the operations of segment dispatch, referrals, runs and placements.
#[derive(Default)]
pub(super) struct Work {
    used: usize,
}

impl Work {
    pub(super) fn pixels(&mut self, pixels: u64, limits: &Limits) -> Result<()> {
        // One decoded/composited pixel plus at most sixteen template samples.
        let cost = pixels
            .checked_mul(18)
            .and_then(|n| usize::try_from(n).ok())
            .filter(|&n| n <= limits.decode_work.saturating_sub(self.used))
            .ok_or_else(|| Error::limit("JBIG2 cumulative pixel work exceeds budget"))?;
        self.used += cost;
        Ok(())
    }

    pub(super) fn operations(&mut self, count: usize, limits: &Limits) -> Result<()> {
        if count > limits.decode_work.saturating_sub(self.used) {
            return Err(Error::limit("JBIG2 cumulative operations exceed budget"));
        }
        self.used += count;
        Ok(())
    }
}
