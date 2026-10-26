//! Owned, backpressured execution over the canonical global-band cursor.

use super::{Charges, PreparedScene};
use crate::execute::{BandCursor, ReadyScratch, Rendering, Scratch, output_band_bytes};
use pagura_core::{Error, MemoryBudget, Reservation, Result, convert::u32_to_usize};
use pagura_raster::DeviceRect;
use std::ops::Range;
use std::sync::Arc;

/// Output allocations admitted for one render, separately from paint budgets.
/// Public fields intentionally permit functional record update, as `Limits` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputLimits {
    /// Live immutable output slots; consumers must release a slot to reuse it.
    pub slots: usize,
    /// Slot capacities, slot headers and the heap-owned active controller.
    /// Excludes the inline `RenderSession` and shared budget-counter allocation;
    /// prepared scene and scratch storage obey the separate paint budgets.
    pub bytes: usize,
}

impl Default for OutputLimits {
    fn default() -> Self {
        Self {
            slots: 2,
            bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
struct BandBuffer {
    page: DeviceRect,
    region: DeviceRect,
    rows: Range<u32>,
    stride: usize,
    pixels: Vec<u8>,
    reservation: Reservation,
}

/// Immutable final rows that pin one preallocated output slot.
///
/// Cloning shares the same slot. Dropping the render session or cancelling it
/// leaves existing leases valid and charged until their final clone is dropped.
#[derive(Debug, Clone)]
pub struct BandLease(Arc<BandBuffer>);

impl BandLease {
    /// Full device-page bounds before selecting the rendered region.
    pub fn page(&self) -> DeviceRect {
        self.0.page
    }
    /// The complete requested region; band rows use these device coordinates.
    pub fn region(&self) -> DeviceRect {
        self.0.region
    }
    /// Device rows in this band, in top-to-bottom order.
    pub fn rows(&self) -> Range<u32> {
        self.0.rows.clone()
    }
    /// Bytes per row, exactly four times the requested region's width.
    pub fn stride(&self) -> usize {
        self.0.stride
    }
    /// Final premultiplied RGBA8 samples in the renderer's sRGB working space.
    /// Zero-alpha pixels follow the existing raster quantization policy.
    pub fn premultiplied_rgba(&self) -> &[u8] {
        &self.0.pixels
    }
    /// Allocation estimate charged once for this slot, independent of clones.
    pub fn retained_bytes(&self) -> usize {
        self.0.reservation.bytes()
    }
}

/// One bounded step of a prepared render. Only `Complete` signifies success.
#[derive(Debug)]
#[non_exhaustive]
pub enum BandOutput {
    /// One final immutable band; the next step continues after its last row.
    Band(BandLease),
    /// Every slot is leased; no band was executed and no cursor advanced.
    Backpressured,
    /// All bands were produced. Outstanding leases may still be consumed.
    Complete,
    /// Cancellation is terminal; outstanding leases remain valid.
    Cancelled,
}

#[derive(Debug)]
struct Active {
    prepared: Arc<PreparedScene>,
    scratch: ReadyScratch,
    cursor: BandCursor,
    slots: Vec<Arc<BandBuffer>>,
    _reservation: Reservation,
}

#[derive(Debug)]
enum State {
    Active(Box<Active>),
    Complete,
    Cancelled,
}

/// An owned prepared scene, scratch and monotonically advancing band cursor.
///
/// Admission performs all allocation. Each `advance` executes at most one band
/// without source reads or allocation. Leases impose backpressure, and neither
/// pausing nor an output wait restarts the rasterizer or active-item lists.
#[derive(Debug)]
pub struct RenderSession {
    state: State,
    output: Arc<MemoryBudget>,
    charges: Charges,
}

impl RenderSession {
    /// Admits scratch and all output slots before any band executes.
    /// The immutable prepared scene may be shared by independent sessions.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for zero slots and `Limit` for output sizing,
    /// output quotas or scratch allocation. No partially admitted session escapes.
    pub fn new(
        prepared: Arc<PreparedScene>,
        scratch: Scratch,
        limits: OutputLimits,
    ) -> Result<Self> {
        if limits.slots == 0 {
            return Err(Error::invalid_argument(
                "render session needs an output slot",
            ));
        }
        let output = MemoryBudget::new(limits.bytes, "render output memory budget exceeded");
        let mut reservation = output.reserve(size_of::<Active>())?;
        let mut slots = Vec::new();
        reservation.reserve(&mut slots, limits.slots)?;
        let band_bytes = output_band_bytes(prepared.plan.area)?;
        let stride = u32_to_usize(prepared.region().width())
            .checked_mul(4)
            .ok_or(Error::limit("render output stride overflow"))?;
        for _ in 0..limits.slots {
            let mut charge = output.reserve(size_of::<BandBuffer>() + 2 * size_of::<usize>())?;
            let mut pixels = Vec::new();
            charge.reserve(&mut pixels, band_bytes)?;
            slots.push(Arc::new(BandBuffer {
                page: prepared.page,
                region: prepared.region(),
                rows: 0..0,
                stride,
                pixels,
                reservation: charge,
            }));
        }
        let mut scratch = scratch.into_ready(&prepared.plan)?;
        let cursor = BandCursor::new(&mut rendering(&prepared, &mut scratch));
        let charges = prepared.charges();
        Ok(Self {
            state: State::Active(Box::new(Active {
                prepared,
                scratch,
                cursor,
                slots,
                _reservation: reservation,
            })),
            output,
            charges,
        })
    }

    /// The unchanged paint budgets reserved by preparation for this region.
    pub fn charges(&self) -> Charges {
        self.charges
    }

    /// Output storage still owned by the session or any outstanding lease.
    /// After terminal completion/cancellation, only consumer-held slots remain.
    /// Fixed inline session and budget-counter overhead are excluded, as in
    /// [`OutputLimits::bytes`]; this is allocation accounting, not RSS.
    pub fn output_bytes(&self) -> usize {
        self.output.used()
    }

    /// Produces one band, an output wait, or the unchanged terminal state.
    pub fn advance(&mut self) -> BandOutput {
        let active = match &mut self.state {
            State::Complete => return BandOutput::Complete,
            State::Cancelled => return BandOutput::Cancelled,
            State::Active(active) => active,
        };
        for slot in &mut active.slots {
            if let Some(buffer) = Arc::get_mut(slot) {
                let mut rendering = rendering(&active.prepared, &mut active.scratch);
                let Some(rows) = active.cursor.rows(&rendering) else {
                    self.state = State::Complete;
                    return BandOutput::Complete;
                };
                buffer.rows = rows;
                buffer.pixels.clear();
                active.cursor.render(&mut rendering, &mut buffer.pixels);
                let band = BandLease(Arc::clone(slot));
                if active.cursor.complete() {
                    self.state = State::Complete;
                }
                return BandOutput::Band(band);
            }
        }
        BandOutput::Backpressured
    }

    /// Cancels an active session and releases all producer-owned storage.
    /// Repeated cancellation is inert; successful completion stays successful.
    pub fn cancel(&mut self) {
        if matches!(self.state, State::Active(_)) {
            self.state = State::Cancelled;
        }
    }
}

fn rendering<'a>(prepared: &'a PreparedScene, scratch: &'a mut ReadyScratch) -> Rendering<'a> {
    Rendering {
        plan: &prepared.plan,
        shapes: &prepared.shapes,
        workspace: &mut scratch.workspace,
        rasterizer: &mut scratch.rasterizer,
    }
}
