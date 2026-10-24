//! Optional original-control byte hints within a completed inspection's allowance.
//!
//! This accounting draft deliberately declines every optional query. A later
//! mathematical implementation must own only child-metered scratch and release
//! it all before returning to the original sequential certificate collector.

use super::{Pixel, Source};
use crate::band::Prep;

pub(super) fn hint(
    _source: Source<'_>,
    _options: &Prep<'_>,
    _pending: &mut [Pixel],
    _proven: &[Pixel],
    _surplus: usize,
    _remaining_bytes: usize,
) {
}
