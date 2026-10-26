//! Domain quotas and conservative admission for the session's owned jobs.

use pagura::source::{LoadingLimits, SourceLimits};
use pagura::{Error, Limits, OutputLimits, RenderJobLimits, Result, png};

/// Progressive session quotas. Public fields intentionally support functional
/// record update. Engine limits still constrain each operation independently.
/// Admission reserves domain ceilings, not measured RSS; source leases and
/// contexts keep their reservations until their last dependent job releases them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressiveLimits {
    /// Registered source handles. Closed sources may still pin byte admission.
    pub sources: usize,
    /// Job handles, including terminal handles awaiting explicit close.
    pub jobs: usize,
    /// Sum of admitted document and active-operation domain ceilings.
    pub active_bytes: usize,
    /// Maximum supplied bytes in one request, before source copying.
    pub supply_bytes: usize,
    /// Individual registered source quotas. Their retained-byte ceilings share
    /// the existing session total_source_bytes admission with complete opens.
    pub source: SourceLimits,
    /// Source-window and cumulative parser work quotas.
    pub loading: LoadingLimits,
    /// Scene-attempt quota until the interpreter retains instruction state.
    pub render: RenderJobLimits,
    /// Canonical band storage. The protocol permits one unacknowledged output.
    pub output: OutputLimits,
    /// Canonical PNG storage/work. Output must be acknowledged before advancing.
    pub encoding: png::EncodingLimits,
}

impl Default for ProgressiveLimits {
    fn default() -> Self {
        Self {
            sources: 16,
            jobs: 32,
            active_bytes: 3 * 1024 * 1024 * 1024,
            supply_bytes: 1024 * 1024,
            source: SourceLimits::default(),
            loading: LoadingLimits::default(),
            render: RenderJobLimits::default(),
            output: OutputLimits {
                slots: 1,
                ..OutputLimits::default()
            },
            encoding: png::EncodingLimits {
                output_slots: 1,
                ..png::EncodingLimits::default()
            },
        }
    }
}

impl ProgressiveLimits {
    /// Opening retains its conservative admission with the resulting context:
    /// credential construction/retention, parsed objects, an input/decoded stream,
    /// a parser window and two store
    /// ceilings (retained values and the context's reusable scratch).
    pub(crate) fn opening_bytes(self, engine: &Limits, store: usize) -> Result<usize> {
        sum(&[
            engine.string_bytes,
            engine.string_bytes,
            engine.document_bytes,
            engine.stream_bytes,
            engine.decoded_bytes,
            self.loading.window_bytes,
            store,
            store,
        ])
    }

    /// Recording and preparation each own their existing render ceiling;
    /// decoder/decoded values and output/encoder buffers retain separate quotas.
    pub(crate) fn render_bytes(self, engine: &Limits) -> Result<usize> {
        sum(&[
            engine.render_bytes,
            engine.render_bytes,
            engine.decode_bytes,
            engine.decoded_bytes,
            self.output.bytes,
            self.encoding.retained_bytes,
        ])
    }
}

fn sum(values: &[usize]) -> Result<usize> {
    values.iter().try_fold(0_usize, |sum, bytes| {
        sum.checked_add(*bytes)
            .ok_or(Error::limit("job admission byte count overflow"))
    })
}
