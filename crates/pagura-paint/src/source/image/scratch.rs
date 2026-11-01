//! Image execution storage: one bounded layout per prepared source, owned
//! and reused by the caller's render scratch.

use super::{ImageSampler, Paint, Plan, sampling, texels};
use pagura_core::{Error, Result};

/// Capacities of one image's execution storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    plan: sampling::Layout,
    mask: Option<sampling::Layout>,
    memo: Option<texels::MemoLayout>,
    summed: usize,
    fused: usize,
}

/// Every mutable image buffer, held outside the prepared scene.
#[derive(Debug)]
pub(crate) struct Scratch {
    pub(super) layout: Layout,
    pub(super) plan: sampling::Scratch,
    pub(super) mask: Option<sampling::Scratch>,
    pub(super) memo: Option<Box<texels::Memo>>,
    pub(super) summed: Vec<f64>,
    pub(super) fused: Vec<f64>,
    /// How the texels of the rows `plan` reads were converted
    /// ([`Self::select`]).
    mode: texels::TexelMode,
}

impl Layout {
    /// The allocations this layout retains, including the boxed value.
    pub(crate) fn bytes(self) -> usize {
        size_of::<Scratch>()
            .saturating_add(self.plan.bytes())
            .saturating_add(self.mask.map_or(0, sampling::Layout::bytes))
            .saturating_add(self.memo.map_or(0, texels::MemoLayout::bytes))
            .saturating_add(
                self.summed
                    .saturating_add(self.fused)
                    .saturating_mul(size_of::<f64>()),
            )
    }

    /// Allocates capacities whose bytes the caller has charged already.
    pub(crate) fn make(self) -> Result<Scratch> {
        Ok(Scratch {
            layout: self,
            plan: self.plan.make()?,
            mask: self.mask.map(sampling::Layout::make).transpose()?,
            memo: self
                .memo
                .map(|layout| layout.make().map(Box::new))
                .transpose()?,
            summed: allocate(self.summed)?,
            fused: allocate(self.fused)?,
            mode: texels::TexelMode::Exact,
        })
    }
}

impl Scratch {
    /// Reads rows of texels converted by `mode` from now on
    /// ([`sampling::Scratch::select`]).
    #[inline]
    pub(super) fn select(&mut self, mode: texels::TexelMode) {
        if self.mode != mode {
            self.plan.select(self.mode.index(), mode.index());
            self.mode = mode;
        }
    }

    /// The capacities this storage was allocated for.
    pub(crate) fn layout(&self) -> Layout {
        self.layout
    }

    /// Bytes retained by this fixed layout.
    pub(crate) fn bytes(&self) -> usize {
        size_of::<Self>()
            + self.plan.bytes()
            + self.mask.as_ref().map_or(0, sampling::Scratch::bytes)
            + self.memo.as_ref().map_or(0, |memo| memo.bytes())
            + (self.summed.capacity() + self.fused.capacity()) * size_of::<f64>()
    }

    /// Invalidates source-dependent caches between renders.
    pub(crate) fn reset(&mut self) {
        self.switch();
        if let Some(memo) = &mut self.memo {
            memo.reset();
        }
    }

    /// Invalidates the caches of one placement before another draw of the
    /// same image takes this storage; their texels, and so the memo, are
    /// the same.
    pub(crate) fn switch(&mut self) {
        self.plan.reset();
        if let Some(mask) = &mut self.mask {
            mask.reset();
        }
    }
}

impl ImageSampler {
    /// The storage needed by this prepared image, independent of cache state.
    pub(crate) fn scratch_layout(&self) -> Layout {
        let Some(prepared) = self.prepared.as_ref() else {
            return Layout {
                plan: sampling::Layout::Points(0),
                mask: None,
                memo: None,
                summed: 0,
                fused: 0,
            };
        };
        let (sampling, paint) = match &prepared.plan {
            Plan::Codes(codes) => {
                return Layout {
                    plan: sampling::Layout::Separable(codes.layout()),
                    mask: None,
                    memo: None,
                    summed: 0,
                    fused: 0,
                };
            }
            Plan::Texels { sampling, paint } => (sampling, paint),
        };
        let fused = prepared
            .mask
            .as_ref()
            .is_some_and(|mask| mask.independent.is_none());
        let lanes = self.channels + 2 + usize::from(fused);
        let values = lanes - usize::from(paint.opaque(&prepared.image.image)) * 2;
        let plan = sampling.layout(values);
        let memo = match paint {
            Paint::Colors(paint) => match paint.1 {
                texels::Strategy::Memo(layout) => Some(layout),
                _ => None,
            },
            Paint::Stencil(_) => None,
        };
        Layout {
            plan,
            mask: prepared
                .mask
                .as_ref()
                .and_then(|mask| mask.independent.as_ref())
                .map(|(_, plan)| plan.layout(1)),
            memo,
            summed: prepared.columns.saturating_mul(lanes),
            fused: if fused {
                usize::try_from(prepared.image.image.width)
                    .unwrap_or(usize::MAX)
                    .saturating_mul(self.channels + 2)
            } else {
                0
            },
        }
    }
}

/// The first `len` values of `values`, grown with defaults where it holds
/// fewer. It never shrinks: a span's values are written before they are
/// read, and a short span between long ones must not make the next long
/// one fill its storage again.
pub(super) fn grown<T: Copy + Default>(values: &mut Vec<T>, len: usize) -> &mut [T] {
    if values.len() < len {
        values.resize(len, T::default());
    }
    &mut values[..len]
}

/// Reserves an allocation after its layout has been charged.
pub(super) fn allocate<T>(count: usize) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| Error::limit("image scratch allocation failed"))?;
    Ok(values)
}
