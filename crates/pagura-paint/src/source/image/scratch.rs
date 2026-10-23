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
        })
    }
}

impl Scratch {
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
        self.plan.reset();
        if let Some(mask) = &mut self.mask {
            mask.reset();
        }
        if let Some(memo) = &mut self.memo {
            memo.reset();
        }
    }
}

impl ImageSampler {
    /// The storage needed by this prepared image, independent of cache state.
    pub(crate) fn scratch_layout(&self) -> Layout {
        let Some(prepared) = self.prepared.as_ref() else {
            return Layout {
                plan: sampling::Layout::Points,
                mask: None,
                memo: None,
                summed: 0,
                fused: 0,
            };
        };
        let fused = prepared
            .mask
            .as_ref()
            .is_some_and(|mask| mask.independent.is_none());
        let lanes = self.channels + 2 + usize::from(fused);
        let values = lanes - usize::from(prepared.opaque()) * 2;
        let plan = match &prepared.plan {
            Plan::Codes(plan) => sampling::Layout::Separable(plan.layout()),
            Plan::Texels(plan) => plan.layout(values),
        };
        let memo = match &prepared.paint {
            Paint::Colors(paint) => match paint.1 {
                texels::Strategy::Memo(layout) => Some(layout),
                _ => None,
            },
            Paint::Stencil { .. } => None,
        };
        Layout {
            plan,
            mask: prepared
                .mask
                .as_ref()
                .and_then(|mask| mask.independent.as_ref())
                .map(|(_, plan)| plan.layout(1)),
            memo,
            summed: if matches!(prepared.plan, Plan::Texels(_)) {
                prepared.columns.saturating_mul(lanes)
            } else {
                0
            },
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

/// Reserves an allocation after its layout has been charged.
pub(super) fn allocate<T>(count: usize) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| Error::limit("image scratch allocation failed"))?;
    Ok(values)
}
