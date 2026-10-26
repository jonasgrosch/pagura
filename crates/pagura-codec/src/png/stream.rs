//! Pull-driven PNG framing over bounded rows and the canonical zlib encoder.

use super::{ColorType, Options, chunk, filter_row};
use crate::encoding::{Bytes, limit, reserved};
use crate::flate::{Compressor, Encoded, Level};
use pagura_core::{Error, Limits, MemoryBudget, Meter, Position, Reservation, Result};
use std::ops::Range;
use std::sync::Arc;

const IDAT: usize = 65536;

/// Streaming encoding quotas, independent of decoded images and rendering.
/// Public fields intentionally support functional record update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodingLimits {
    /// Logical input pixels; no full image allocation is implied.
    pub pixels: usize,
    /// Working vectors, output slots and heap controller allocation estimates.
    /// Fixed inline `Encoder` and shared counter overhead are excluded.
    pub retained_bytes: usize,
    /// Conservative maximum complete PNG bytes, including all framing.
    pub output_bytes: usize,
    /// Upper bound on matching, filtering and code-construction work units.
    pub work: u64,
    /// Preallocated immutable chunk slots held by downstream consumers.
    pub output_slots: usize,
}

impl Default for EncodingLimits {
    fn default() -> Self {
        let limits = Limits::default();
        Self {
            pixels: limits.pixels,
            retained_bytes: 16 * 1024 * 1024,
            output_bytes: limits.stream_bytes,
            work: 1 << 46,
            output_slots: 2,
        }
    }
}

pub(super) struct Geometry {
    width: u32,
    height: u32,
    channels: usize,
    color: ColorType,
    row: usize,
    input: usize,
    filtered: usize,
}

impl Geometry {
    pub(super) fn new(width: u32, height: u32, color: ColorType, pixels: usize) -> Result<Self> {
        if width == 0 || height == 0 || width > 0x7fff_ffff || height > 0x7fff_ffff {
            return Err(Error::invalid_argument("invalid PNG dimensions").at(Position::Buffer(0)));
        }
        let columns = usize::try_from(width).map_err(|_| limit(0))?;
        let rows = usize::try_from(height).map_err(|_| limit(0))?;
        let count = columns
            .checked_mul(rows)
            .filter(|&n| n <= pixels)
            .ok_or_else(|| limit(0))?;
        let channels = color.channels();
        let input = count.checked_mul(channels).ok_or_else(|| limit(0))?;
        let row = columns.checked_mul(channels).ok_or_else(|| limit(0))?;
        let filtered = input.checked_add(rows).ok_or_else(|| limit(0))?;
        u64::try_from(row)
            .ok()
            .and_then(|n| n.checked_mul(128))
            .ok_or_else(|| limit(0))?;
        Ok(Self {
            width,
            height,
            channels,
            color,
            row,
            input,
            filtered,
        })
    }

    pub(super) fn input_bytes(&self) -> usize {
        self.input
    }

    fn output_bound(&self) -> Result<usize> {
        let blocks = self.filtered.div_ceil(65535).max(1);
        let zlib = blocks
            .checked_mul(5)
            .and_then(|n| self.filtered.checked_add(n))
            .and_then(|n| n.checked_add(6))
            .ok_or_else(|| limit(0))?;
        zlib.div_ceil(IDAT)
            .checked_mul(12)
            .and_then(|n| n.checked_add(zlib))
            .and_then(|n| n.checked_add(45))
            .ok_or_else(|| limit(0))
    }

    fn work_bound(&self, level: Level) -> Result<u64> {
        let input = u64::try_from(self.filtered).map_err(|_| limit(0))?;
        let depth = if level == Level::Fast { 32 } else { 128 };
        let per_byte = 2 * depth * 258 + 64;
        input
            .checked_mul(per_byte)
            .and_then(|n| {
                input
                    .div_ceil(65535)
                    .checked_mul(1_000_000)
                    .and_then(|codes| n.checked_add(codes))
            })
            .ok_or_else(|| limit(0))
    }
}

struct Rows {
    current: Vec<u8>,
    previous: Vec<u8>,
    candidate: Vec<u8>,
    best: Vec<u8>,
    tag: u8,
    pending: Option<usize>,
}

impl Rows {
    fn new(width: usize, bytes: &mut Meter) -> Result<Self> {
        let mut current = reserved(width, 0, bytes)?;
        current.clear();
        Ok(Self {
            current,
            previous: reserved(width, 0, bytes)?,
            candidate: reserved(width, 0, bytes)?,
            best: reserved(width, 0, bytes)?,
            tag: 0,
            pending: None,
        })
    }

    fn write(&mut self, input: &[u8], geometry: &Geometry) -> usize {
        if self.pending.is_some() {
            return 0;
        }
        let count = input.len().min(geometry.row - self.current.len());
        self.current.extend_from_slice(&input[..count]);
        if self.current.len() == geometry.row {
            self.tag = filter_row(
                &self.current,
                &self.previous,
                geometry.channels,
                &mut self.candidate,
                &mut self.best,
            );
            std::mem::swap(&mut self.current, &mut self.previous);
            self.current.clear();
            self.pending = Some(0);
        }
        count
    }

    fn feed(&mut self, compressor: &mut Compressor) -> Result<()> {
        let Some(mut offset) = self.pending else {
            return Ok(());
        };
        if offset == 0 {
            offset += compressor.push(&[self.tag])?;
            if offset == 0 {
                return Ok(());
            }
        }
        offset += compressor.push(&self.best[offset - 1..])?;
        self.pending = (offset <= self.best.len()).then_some(offset);
        Ok(())
    }
}

struct Chunk {
    output: Bytes,
    reservation: Reservation,
}

/// Immutable PNG framing bytes, pinned to one preallocated output slot.
/// Clones share the same allocation and charge, including after cancellation.
#[derive(Clone)]
pub struct ChunkLease(Arc<Chunk>);

impl std::fmt::Debug for ChunkLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChunkLease")
            .field("length", &self.bytes().len())
            .finish_non_exhaustive()
    }
}

impl ChunkLease {
    /// A complete signature/header, IDAT chunk or terminal IEND chunk.
    pub fn bytes(&self) -> &[u8] {
        &self.0.output.data
    }
    /// Allocation estimate for this slot, charged once regardless of clones.
    pub fn retained_bytes(&self) -> usize {
        self.0.reservation.bytes()
    }
}

/// One bounded encoding step; only `Complete` signifies successful completion.
#[derive(Debug)]
#[non_exhaustive]
pub enum EncodingOutput {
    /// One immutable portion of the single PNG byte stream.
    Chunk(ChunkLease),
    /// Supply more pixel bytes through [`Encoder::write`].
    NeedInput,
    /// A canonical block progressed without filling the next output chunk.
    Yielded,
    /// All output slots are held; no encoding work occurred.
    Backpressured,
    /// The single zlib checksum and IEND were produced exactly once.
    Complete,
    /// Cancellation is terminal and never produces successful completion.
    Cancelled,
}

enum Emission {
    Chunk,
    Final,
    NeedInput,
    Yielded,
}

struct Active {
    geometry: Geometry,
    rows: Rows,
    compressor: Compressor,
    compressed: Range<usize>,
    idat: Vec<u8>,
    header: bool,
    received: usize,
    emitted: usize,
    output_limit: usize,
    slots: Vec<Arc<Chunk>>,
    _reservation: Reservation,
}

impl Active {
    fn allocate(
        geometry: Geometry,
        level: Level,
        limits: EncodingLimits,
        budget: &Arc<MemoryBudget>,
    ) -> Result<Self> {
        let mut bytes = Meter::new(
            limits.retained_bytes,
            "encoding retained-memory budget exceeded",
        );
        bytes.charge(size_of::<Self>())?;
        let rows = Rows::new(geometry.row, &mut bytes)?;
        let compressor = Compressor::new(geometry.filtered, level, &mut bytes)?;
        let mut idat = reserved(IDAT, 0, &mut bytes)?;
        idat.clear();
        let mut slots = Vec::new();
        bytes.reserve_capacity(&mut slots, limits.output_slots)?;
        let reservation = budget.reserve(bytes.used())?;
        for _ in 0..limits.output_slots {
            let mut charge = budget.reserve(size_of::<Chunk>() + 2 * size_of::<usize>())?;
            let mut data = Vec::new();
            charge.reserve(&mut data, IDAT + 12)?;
            slots.push(Arc::new(Chunk {
                output: Bytes::from_reserved(data, IDAT + 12),
                reservation: charge,
            }));
        }
        Ok(Self {
            geometry,
            rows,
            compressor,
            compressed: 0..0,
            idat,
            header: true,
            received: 0,
            emitted: 0,
            output_limit: limits.output_bytes,
            slots,
            _reservation: reservation,
        })
    }

    fn write(&mut self, bytes: &[u8]) -> Result<usize> {
        if bytes.len() > self.geometry.input.saturating_sub(self.received) {
            return Err(Error::invalid_argument(
                "PNG input exceeds declared dimensions",
            ));
        }
        if !self.slots.iter().any(|slot| Arc::strong_count(slot) == 1) {
            return Ok(0);
        }
        let count = self.rows.write(bytes, &self.geometry);
        self.received += count;
        Ok(count)
    }

    fn fill_idat(&mut self) {
        let count = self.compressed.len().min(IDAT - self.idat.len());
        let end = self.compressed.start + count;
        self.idat
            .extend_from_slice(&self.compressor.output()[self.compressed.start..end]);
        self.compressed.start = end;
    }

    fn emit(&mut self, output: &mut Bytes) -> Result<Emission> {
        output.data.clear();
        let result = if self.header {
            output.extend(b"\x89PNG\r\n\x1a\n")?;
            let mut ihdr = [0; 13];
            ihdr[..4].copy_from_slice(&self.geometry.width.to_be_bytes());
            ihdr[4..8].copy_from_slice(&self.geometry.height.to_be_bytes());
            ihdr[8] = 8;
            ihdr[9] = self.geometry.color.code();
            chunk(output, *b"IHDR", &ihdr)?;
            self.header = false;
            Emission::Chunk
        } else {
            self.data_step(output)?
        };
        self.emitted = self
            .emitted
            .checked_add(output.data.len())
            .filter(|&n| n <= self.output_limit)
            .ok_or_else(|| limit(self.emitted))?;
        Ok(result)
    }

    fn data_step(&mut self, output: &mut Bytes) -> Result<Emission> {
        self.fill_idat();
        if self.idat.len() == IDAT || (self.compressor.finished() && self.compressed.is_empty()) {
            if self.idat.is_empty() {
                chunk(output, *b"IEND", &[])?;
                return Ok(Emission::Final);
            }
            chunk(output, *b"IDAT", &self.idat)?;
            self.idat.clear();
            return Ok(Emission::Chunk);
        }
        self.rows.feed(&mut self.compressor)?;
        match self.compressor.advance()? {
            Encoded::NeedInput => Ok(Emission::NeedInput),
            Encoded::Bytes(bytes) => {
                self.compressed = 0..bytes.len();
                self.fill_idat();
                if self.idat.len() == IDAT {
                    chunk(output, *b"IDAT", &self.idat)?;
                    self.idat.clear();
                    Ok(Emission::Chunk)
                } else {
                    Ok(Emission::Yielded)
                }
            }
            Encoded::Complete => Ok(Emission::Yielded),
        }
    }
}

enum State {
    Active(Box<Active>),
    Complete,
    Cancelled,
    Failed(Error),
}

/// A bounded PNG encoder: partial pixel input, one zlib stream and leased output.
///
/// All storage and conservative work/output bounds are admitted before the first
/// chunk. Supply tightly packed straight samples; `write` may accept a prefix.
/// Each `advance` performs at most one canonical DEFLATE block. Output bytes do
/// not depend on input splits, output demand or renderer band height.
pub struct Encoder {
    state: State,
    budget: Arc<MemoryBudget>,
}

impl std::fmt::Debug for Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoder")
            .field("retained_bytes", &self.retained_bytes())
            .finish_non_exhaustive()
    }
}

impl Encoder {
    /// Admits a deterministic Gray/RGB/straight-RGBA PNG before output begins.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for invalid dimensions or zero slots, and
    /// `Limit` for pixels, memory, conservative output/work bounds or allocation.
    /// Streaming admission can reject a quota that a more compressed image
    /// might actually fit; the complete-buffer collector keeps its exact quota.
    pub fn new(
        width: u32,
        height: u32,
        color: ColorType,
        level: Level,
        limits: EncodingLimits,
    ) -> Result<Self> {
        let geometry = Geometry::new(width, height, color, limits.pixels)?;
        if geometry.output_bound()? > limits.output_bytes {
            return Err(limit(0));
        }
        Self::admit(geometry, level, limits)
    }

    pub(super) fn collecting(geometry: Geometry, options: &Options) -> Result<Self> {
        if options.limits.stream_bytes < 65 {
            return Err(limit(0));
        }
        Self::admit(
            geometry,
            options.level,
            EncodingLimits {
                retained_bytes: usize::MAX,
                output_bytes: options.limits.stream_bytes,
                work: u64::MAX,
                output_slots: 1,
                ..EncodingLimits::default()
            },
        )
    }

    fn admit(geometry: Geometry, level: Level, limits: EncodingLimits) -> Result<Self> {
        if limits.output_slots == 0 {
            return Err(Error::invalid_argument("PNG encoder needs an output slot"));
        }
        if geometry.work_bound(level)? > limits.work {
            return Err(limit(0));
        }
        let budget = MemoryBudget::new(
            limits.retained_bytes,
            "encoding retained-memory budget exceeded",
        );
        let active = Active::allocate(geometry, level, limits, &budget)?;
        Ok(Self {
            state: State::Active(Box::new(active)),
            budget,
        })
    }

    /// Working/output allocation estimates, including all consumer-held chunks.
    /// Terminal jobs retain only leased slots; fixed inline/counter overhead is excluded.
    pub fn retained_bytes(&self) -> usize {
        self.budget.used()
    }

    /// Accepts a bounded prefix of the next straight pixel bytes, returning its length.
    /// A zero result requires `advance` or release of a held output lease.
    ///
    /// # Errors
    /// Returns terminal `InvalidArgument` for excess input or writes after a
    /// terminal state. Partial rows are retained; no padding is invented.
    pub fn write(&mut self, bytes: &[u8]) -> Result<usize> {
        let result = match &mut self.state {
            State::Active(active) => active.write(bytes),
            State::Failed(error) => return Err(error.clone()),
            State::Complete | State::Cancelled => {
                return Err(Error::invalid_argument("PNG encoder is terminal"));
            }
        };
        if let Err(error) = &result {
            self.state = State::Failed(error.clone());
        }
        result
    }

    /// Emits a chunk or reports input/output demand, progress or a terminal state.
    ///
    /// # Errors
    /// Returns a terminal encoding failure. Ordinary resource failures are
    /// rejected during admission; no successful completion follows a failure.
    pub fn advance(&mut self) -> Result<EncodingOutput> {
        let active = match &mut self.state {
            State::Active(active) => active,
            State::Complete => return Ok(EncodingOutput::Complete),
            State::Cancelled => return Ok(EncodingOutput::Cancelled),
            State::Failed(error) => return Err(error.clone()),
        };
        let Some(index) = active
            .slots
            .iter()
            .position(|slot| Arc::strong_count(slot) == 1)
        else {
            return Ok(EncodingOutput::Backpressured);
        };
        // Temporarily move the existing Arc out without allocating. No slot is
        // published during this call, and the private slot has no weak owners.
        let mut slot = active.slots.swap_remove(index);
        let Some(buffer) = Arc::get_mut(&mut slot) else {
            active.slots.push(slot);
            return Ok(EncodingOutput::Backpressured);
        };
        let result = active.emit(&mut buffer.output);
        match result {
            Ok(Emission::Chunk) => {
                let lease = ChunkLease(Arc::clone(&slot));
                active.slots.push(slot);
                Ok(EncodingOutput::Chunk(lease))
            }
            Ok(Emission::Final) => {
                let lease = ChunkLease(slot);
                self.state = State::Complete;
                Ok(EncodingOutput::Chunk(lease))
            }
            Ok(Emission::NeedInput) => {
                active.slots.push(slot);
                Ok(EncodingOutput::NeedInput)
            }
            Ok(Emission::Yielded) => {
                active.slots.push(slot);
                Ok(EncodingOutput::Yielded)
            }
            Err(error) => {
                self.state = State::Failed(error.clone());
                Err(error)
            }
        }
    }

    /// Cancels active work, dropping all producer storage without emitting IEND.
    /// Completed or failed encoders keep their existing terminal state.
    pub fn cancel(&mut self) {
        if matches!(self.state, State::Active(_)) {
            self.state = State::Cancelled;
        }
    }
}
