use super::memory::{Budget, Retained, Storage};
use super::types::Ranges;
use super::{
    ByteLease, ByteRange, ByteSource, NeedData, ReadFault, ReadResult, SourceIdentity, SourceLimits,
};
use pagura_core::{Error, INDEX_ENTRY_OVERHEAD, Limits, Meter, Result};
use std::{
    collections::BTreeMap,
    ops::Range,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

#[derive(Debug)]
struct Chunk {
    end: usize,
    backing: Arc<Retained<Storage>>,
}

#[derive(Debug)]
struct Available {
    chunks: BTreeMap<usize, Chunk>,
    generation: u64,
    work: Meter,
    terminal: Option<Error>,
}

impl Available {
    fn active(&self) -> Result<()> {
        self.terminal
            .as_ref()
            .map_or(Ok(()), |error| Err(error.clone()))
    }
}

/// Result of one atomic supply. Duplicate bytes add no progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Supply {
    added_bytes: usize,
    generation: u64,
}

impl Supply {
    /// Previously unavailable bytes published by this supply.
    pub const fn added_bytes(self) -> usize {
        self.added_bytes
    }
    /// The availability generation after this supply.
    pub const fn generation(self) -> u64 {
        self.generation
    }
}

/// A bounded source filled by verified caller-supplied ranges.
///
/// Accepted bytes never change. Overlap is checked before publishing any new
/// fragment. Conflicting content permanently invalidates subsequent operations;
/// unrelated identities are rejected without changing this source.
#[derive(Debug)]
pub struct SparseBytes {
    identity: SourceIdentity,
    length: u64,
    limits: SourceLimits,
    budget: Arc<Budget>,
    available: Mutex<Available>,
}

impl SparseBytes {
    /// Creates an empty availability map for a fixed representation and length.
    ///
    /// # Errors
    /// Returns `Limit` if the declared file length exceeds the configured or target limit.
    pub fn new(
        identity: SourceIdentity,
        length: u64,
        limits: &Limits,
        source_limits: &SourceLimits,
    ) -> Result<Self> {
        super::length(length, limits)?;
        Ok(Self {
            identity,
            length,
            limits: *source_limits,
            budget: Budget::new(source_limits.retained_bytes),
            available: Mutex::new(Available {
                chunks: BTreeMap::new(),
                generation: 0,
                terminal: None,
                work: Meter::new(source_limits.work, "source work budget exceeded"),
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Available> {
        self.available
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Copies and atomically publishes only the previously unavailable bytes.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for a different identity, out-of-range supply,
    /// closed source or conflicting overlap, and `Limit` for checked arithmetic,
    /// work, fragment count or retained storage. Failure publishes no new bytes.
    pub fn supply(&self, identity: SourceIdentity, offset: u64, bytes: &[u8]) -> Result<Supply> {
        if identity != self.identity {
            return Err(Error::invalid_argument(
                "supply belongs to another source identity",
            ));
        }
        let size = u64::try_from(bytes.len())
            .map_err(|_| Error::limit("source supply length overflow"))?;
        let end = offset
            .checked_add(size)
            .ok_or(Error::limit("source supply offset overflow"))?;
        let range = ByteRange::new(offset, end)?.offsets(self.length)?;
        let mut available = self.lock();
        available.active()?;
        available.work.charge(
            bytes
                .len()
                .checked_add(1)
                .ok_or(Error::limit("source supply work overflow"))?,
        )?;
        if !matches(&mut available, range.clone(), bytes)? {
            let error = Error::invalid_argument("source supply conflicts with accepted bytes");
            available.terminal = Some(error.clone());
            return Err(error);
        }
        let Some(gaps) = self.gaps(&mut available, range.clone(), self.limits.chunks)? else {
            return Ok(Supply {
                added_bytes: 0,
                generation: available.generation,
            });
        };
        available
            .chunks
            .len()
            .checked_add(gaps.value.len())
            .filter(|&count| count <= self.limits.chunks)
            .ok_or(Error::limit("source fragment count exceeded"))?;
        let generation = available
            .generation
            .checked_add(1)
            .ok_or(Error::limit("source availability generation exhausted"))?;
        let (mut prepared, _reservation) = self.budget.vector(gaps.value.len())?;
        let mut added_bytes = 0_usize;
        for gap in &gaps.value {
            let part = gap.offsets(self.length)?;
            let input = &bytes[part.start - range.start..part.end - range.start];
            let (mut copied, mut reservation) = self.budget.vector(input.len())?;
            reservation.grow(size_of::<(usize, Chunk)>() + INDEX_ENTRY_OVERHEAD)?;
            copied.extend_from_slice(input);
            added_bytes = added_bytes.saturating_add(input.len());
            prepared.push((
                part.start,
                Chunk {
                    end: part.end,
                    backing: Retained::new(Storage::Owned(copied), reservation),
                },
            ));
        }
        for (start, chunk) in prepared {
            available.chunks.insert(start, chunk);
        }
        available.generation = generation;
        Ok(Supply {
            added_bytes,
            generation,
        })
    }

    /// Closes this source and drops its map; outstanding leases remain valid and charged.
    pub fn close(&self) {
        let mut available = self.lock();
        available.terminal = Some(Error::invalid_argument("source is closed"));
        available.chunks.clear();
    }

    fn gaps(
        &self,
        available: &mut Available,
        range: Range<usize>,
        cap: usize,
    ) -> Result<Option<Ranges>> {
        let mut gaps = None;
        let Available { chunks, work, .. } = available;
        let mut next = range.start;
        for (&start, chunk) in overlapping(chunks, range.clone()) {
            work.charge(1)?;
            if next < start {
                push_gap(&self.budget, &mut gaps, next..start.min(range.end), cap)?;
            }
            next = next.max(chunk.end).min(range.end);
        }
        if next < range.end {
            push_gap(&self.budget, &mut gaps, next..range.end, cap)?;
        }
        Ok(gaps.map(|(ranges, reservation)| Retained::new(ranges, reservation)))
    }

    fn assemble(
        &self,
        available: &mut Available,
        range: Range<usize>,
        original: ByteRange,
    ) -> Result<ByteLease> {
        if range.len() > self.limits.assembly_bytes {
            return Err(Error::limit("source assembly window budget exceeded"));
        }
        available.work.charge(range.len())?;
        let (mut bytes, reservation) = self.budget.vector(range.len())?;
        for (&start, chunk) in overlapping(&available.chunks, range.clone()) {
            let begin = start.max(range.start) - start;
            let end = chunk.end.min(range.end) - start;
            bytes.extend_from_slice(&chunk.backing.value.bytes()[begin..end]);
        }
        debug_assert_eq!(bytes.len(), range.len());
        Ok(ByteLease {
            backing: Some(Retained::new(Storage::Owned(bytes), reservation)),
            extent: 0..range.len(),
            range: original,
        })
    }
}

impl ByteSource for SparseBytes {
    fn identity(&self) -> SourceIdentity {
        self.identity
    }
    fn len(&self) -> u64 {
        self.length
    }
    fn generation(&self) -> u64 {
        self.lock().generation
    }
    fn retained_bytes(&self) -> usize {
        self.budget.used()
    }
    fn read_exact(&self, range: ByteRange) -> ReadResult<ByteLease> {
        let extent = range.offsets(self.length)?;
        let mut available = self.lock();
        available.active()?;
        available.work.charge(1)?;
        if extent.is_empty() {
            return Ok(ByteLease::empty(range));
        }
        if let Some((&start, chunk)) = available.chunks.range(..=extent.start).next_back()
            && extent.end <= chunk.end
        {
            return Ok(ByteLease {
                backing: Some(Arc::clone(&chunk.backing)),
                extent: extent.start - start..extent.end - start,
                range,
            });
        }
        if let Some(gaps) = self.gaps(&mut available, extent.clone(), self.limits.missing_ranges)? {
            return Err(ReadFault::Unavailable(NeedData {
                identity: self.identity,
                generation: available.generation,
                ranges: gaps,
            }));
        }
        self.assemble(&mut available, extent, range)
            .map_err(ReadFault::from)
    }
}

fn overlapping(
    chunks: &BTreeMap<usize, Chunk>,
    range: Range<usize>,
) -> impl Iterator<Item = (&usize, &Chunk)> {
    let start = chunks
        .range(..=range.start)
        .next_back()
        .map_or(range.start, |(&start, _)| start);
    chunks
        .range(start..range.end)
        .filter(move |(_, chunk)| chunk.end > range.start)
}

fn matches(available: &mut Available, range: Range<usize>, bytes: &[u8]) -> Result<bool> {
    let Available { chunks, work, .. } = available;
    for (&start, chunk) in overlapping(chunks, range.clone()) {
        work.charge(1)?;
        let begin = start.max(range.start);
        let end = chunk.end.min(range.end);
        if chunk.backing.value.bytes()[begin - start..end - start]
            != bytes[begin - range.start..end - range.start]
        {
            return Ok(false);
        }
    }
    Ok(true)
}

type Gaps = Option<(Vec<ByteRange>, super::memory::Reservation)>;

fn push_gap(budget: &Arc<Budget>, gaps: &mut Gaps, range: Range<usize>, cap: usize) -> Result<()> {
    let (gaps, reservation) = if let Some(pair) = gaps {
        pair
    } else {
        gaps.insert(budget.vector(0)?)
    };
    if gaps.len() >= cap {
        return Err(Error::limit("source missing-range count exceeded"));
    }
    reservation.reserve(gaps, 1)?;
    let start = u64::try_from(range.start).map_err(|_| Error::limit("source offset overflow"))?;
    let end = u64::try_from(range.end).map_err(|_| Error::limit("source offset overflow"))?;
    gaps.push(ByteRange::new(start, end)?);
    Ok(())
}
