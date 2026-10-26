//! Immutable complete and sparse byte sources (ADR 0036).
//!
//! Missing bytes are control flow, not PDF syntax errors. Sources perform no
//! I/O. Leases retain their allocation and its budget charge until the last
//! owner releases it, including after a sparse source is closed.

mod complete;
mod memory;
mod sparse;
mod types;

pub use complete::CompleteBytes;
pub use sparse::{SparseBytes, Supply};
pub use types::{ByteLease, ByteRange, NeedData, ReadFault, ReadResult, SourceIdentity};

use pagura_core::{Error, Limits, Result};

/// Limits for one source store, independent of syntax and rendering budgets.
///
/// Public fields intentionally support functional record update. Defaults fit
/// a 32-bit target. The retained-byte count is an allocation estimate, not RSS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLimits {
    /// Retained input, assembled windows and outstanding missing-range records.
    pub retained_bytes: usize,
    /// Maximum bytes copied into one assembled window or materialized source.
    pub assembly_bytes: usize,
    /// Maximum non-overlapping accepted source fragments.
    pub chunks: usize,
    /// Maximum missing intervals returned by one read.
    pub missing_ranges: usize,
    /// Cumulative bytes and fragment visits inspected by supply/read operations.
    pub work: usize,
}

impl Default for SourceLimits {
    fn default() -> Self {
        Self {
            retained_bytes: 128 * 1024 * 1024,
            assembly_bytes: 16 * 1024 * 1024,
            chunks: 4096,
            missing_ranges: 128,
            work: 1024 * 1024 * 1024,
        }
    }
}

/// One fixed byte representation with caller-supplied availability.
///
/// Implementations must never replace accepted bytes. A source error cannot
/// become a repaired PDF or an undefined-reference null.
pub trait ByteSource: std::fmt::Debug + Send + Sync {
    /// The immutable registration and version identity.
    fn identity(&self) -> SourceIdentity;
    /// The fixed declared file length.
    fn len(&self) -> u64;
    /// Whether the declared file is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Advances only when previously unavailable bytes are accepted.
    fn generation(&self) -> u64;
    /// Allocation bytes retained by this source and its outstanding leases.
    fn retained_bytes(&self) -> usize;
    /// Leases exactly `range`, or reports its missing intervals.
    ///
    /// # Errors
    /// Returns unavailable input separately from terminal argument, consistency
    /// and resource failures. A range outside the declared file is invalid.
    fn read_exact(&self, range: ByteRange) -> ReadResult<ByteLease>;
    /// Explicitly obtains all original bytes, charging any assembly copy.
    ///
    /// # Errors
    /// As [`Self::read_exact`]. Fragmented input also obeys the assembly cap;
    /// this operation cannot hide a whole-file allocation behind a slice view.
    fn materialize(&self) -> ReadResult<ByteLease> {
        self.read_exact(ByteRange::whole(self.len()))
    }
}

fn length(length: u64, limits: &Limits) -> Result<usize> {
    usize::try_from(length)
        .ok()
        .filter(|&length| length <= limits.file_bytes)
        .ok_or(Error::limit("source file size budget exceeded"))
}
