use super::memory::{Retained, Storage};
use pagura_core::{Error, Result};
use std::{fmt, ops::Range, sync::Arc};

/// An opaque registration ID and immutable version token, never a URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceIdentity {
    registration: u64,
    version: [u8; 32],
}

impl SourceIdentity {
    /// Creates a caller-assigned identity. Reuse only for the same byte representation.
    pub const fn new(registration: u64, version: [u8; 32]) -> Self {
        Self {
            registration,
            version,
        }
    }
    /// The opaque registration ID.
    pub const fn registration(self) -> u64 {
        self.registration
    }
    /// The opaque version token.
    pub const fn version(self) -> [u8; 32] {
        self.version
    }
}

/// A checked half-open interval of absolute source offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ByteRange {
    start: u64,
    end: u64,
}

impl ByteRange {
    /// Creates an interval, including an empty interval.
    ///
    /// # Errors
    /// Returns `InvalidArgument` when `end` precedes `start`.
    pub fn new(start: u64, end: u64) -> Result<Self> {
        if end < start {
            return Err(Error::invalid_argument("source range end precedes start"));
        }
        Ok(Self { start, end })
    }
    pub(super) const fn whole(end: u64) -> Self {
        Self { start: 0, end }
    }
    /// The first included source offset.
    pub const fn start(self) -> u64 {
        self.start
    }
    /// The first excluded source offset.
    pub const fn end(self) -> u64 {
        self.end
    }
    /// Number of bytes in the interval.
    pub const fn len(self) -> u64 {
        self.end - self.start
    }
    /// Whether the interval contains no bytes.
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
    pub(super) fn offsets(self, length: u64) -> Result<Range<usize>> {
        if self.end > length {
            return Err(Error::invalid_argument(
                "source range outside declared length",
            ));
        }
        let start =
            usize::try_from(self.start).map_err(|_| Error::limit("source offset overflow"))?;
        let end = usize::try_from(self.end).map_err(|_| Error::limit("source offset overflow"))?;
        Ok(start..end)
    }
}

/// Shared immutable bytes and their original absolute source interval.
#[derive(Debug, Clone)]
pub struct ByteLease {
    pub(super) backing: Option<Arc<Retained<Storage>>>,
    pub(super) extent: Range<usize>,
    pub(super) range: ByteRange,
}

impl ByteLease {
    pub(super) fn empty(range: ByteRange) -> Self {
        Self {
            backing: None,
            extent: 0..0,
            range,
        }
    }
    /// The immutable bytes held by this lease.
    pub fn bytes(&self) -> &[u8] {
        self.backing
            .as_ref()
            .map_or(&[], |backing| &backing.value.bytes()[self.extent.clone()])
    }
    /// The absolute interval represented by [`Self::bytes`].
    pub const fn range(&self) -> ByteRange {
        self.range
    }

    /// Shares a contained interval without reading a provider or allocating
    /// another byte buffer. The original reservation stays owned by the lease.
    pub(super) fn slice(&self, range: ByteRange) -> Result<Self> {
        let offsets = range.offsets(self.range.end())?;
        let original = self.range.offsets(self.range.end())?;
        if offsets.start < original.start {
            return Err(Error::invalid_argument(
                "source interval precedes its lease",
            ));
        }
        let start = self
            .extent
            .start
            .checked_add(offsets.start - original.start)
            .ok_or(Error::limit("source lease offset overflow"))?;
        let end = start
            .checked_add(offsets.len())
            .filter(|&end| end <= self.extent.end)
            .ok_or(Error::limit("source lease extent overflow"))?;
        Ok(Self {
            backing: self.backing.clone(),
            extent: start..end,
            range,
        })
    }
}

/// The exact missing intervals of one read at an observed availability generation.
#[derive(Debug, Clone)]
pub struct NeedData {
    pub(super) identity: SourceIdentity,
    pub(super) generation: u64,
    pub(super) ranges: Ranges,
}

pub(super) type Ranges = Arc<Retained<Vec<ByteRange>>>;

impl NeedData {
    /// The source for which these intervals were requested.
    pub const fn identity(&self) -> SourceIdentity {
        self.identity
    }
    /// The availability generation observed by this read.
    pub const fn generation(&self) -> u64 {
        self.generation
    }
    /// Sorted, disjoint intervals, bounded by the source's missing-range limit.
    pub fn ranges(&self) -> &[ByteRange] {
        &self.ranges.value
    }
}

/// Source availability, separate from terminal PDF/argument/resource errors.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ReadFault {
    /// The requested bytes have not arrived; retry only after useful supply.
    Unavailable(NeedData),
    /// A terminal failure of the operation or source.
    Invalid(Error),
}

impl From<Error> for ReadFault {
    fn from(error: Error) -> Self {
        Self::Invalid(error)
    }
}

impl From<ReadFault> for pagura_core::syntax::ParseFault<NeedData> {
    fn from(fault: ReadFault) -> Self {
        match fault {
            ReadFault::Unavailable(missing) => Self::Deferred(missing),
            ReadFault::Invalid(error) => Self::Invalid(error),
        }
    }
}

impl fmt::Display for ReadFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(_) => formatter.write_str("source bytes are unavailable"),
            Self::Invalid(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for ReadFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unavailable(_) => None,
            Self::Invalid(error) => Some(error),
        }
    }
}

/// Result of an operation that may need more source bytes.
pub type ReadResult<T> = std::result::Result<T, ReadFault>;
