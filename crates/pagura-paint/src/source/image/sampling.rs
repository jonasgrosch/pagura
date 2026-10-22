//! One prepared sampling plan for converted texels and mask opacities.
//! Integer code averages share the separable kernel in `separable`.

use super::contour::{self, Contour};
use super::geometry::{Geometry, Mode, Texels};
use super::separable::{self, RowCache, Rows, Separable};
use crate::source::Pixels;
use pagura_core::Result;

/// Exactly one way to sample a placement, selected during preparation.
#[derive(Debug)]
pub(super) enum Sampling {
    /// Nearest or bilinear samples of individual pixels.
    Points,
    /// Upright nearest samples or exact area averages.
    Separable(Separable),
    /// Exact area averages of a turned placement.
    Contour(Contour),
}

/// Capacities compared before render storage is reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Layout {
    Points,
    Separable(separable::Layout),
    Contour(contour::Layout),
}

/// Mutable values owned by the caller, never by a prepared scene.
#[derive(Debug)]
pub(super) enum Scratch {
    Points,
    Separable(Box<separable::Scratch>),
    Contour(contour::Scratch),
}

impl Layout {
    /// Bytes allocated for this sampling plan.
    pub(super) fn bytes(self) -> usize {
        match self {
            Self::Points => 0,
            Self::Separable(layout) => layout.bytes(),
            Self::Contour(layout) => layout.bytes(),
        }
    }

    /// Allocates capacities already charged by the source layout.
    pub(super) fn make(self) -> Result<Scratch> {
        Ok(match self {
            Self::Points => Scratch::Points,
            Self::Separable(layout) => Scratch::Separable(Box::new(layout.make()?)),
            Self::Contour(layout) => Scratch::Contour(layout.make()?),
        })
    }
}

impl Scratch {
    /// Actual capacities held for this plan.
    pub(super) fn bytes(&self) -> usize {
        match self {
            Self::Points => 0,
            Self::Separable(scratch) => scratch.bytes(),
            Self::Contour(scratch) => scratch.bytes(),
        }
    }

    /// Invalidates rows before another render can use this storage.
    pub(super) fn reset(&mut self) {
        if let Self::Separable(scratch) = self {
            scratch.reset();
        }
    }
}

impl Sampling {
    /// Plans `geometry` over device `columns`.
    ///
    /// # Errors
    ///
    /// A `Limit` error when the column plan cannot be allocated.
    pub(super) fn new(geometry: &Geometry, columns: (u32, u32)) -> Result<Self> {
        // Bilinear pixels keep their four products in the same order for
        // every placement, including upright ones.
        if geometry.mode() != Mode::Bilinear
            && let Some(separable) = Separable::new(geometry, columns)?
        {
            return Ok(Self::Separable(separable));
        }
        Ok(if geometry.mode() == Mode::Area {
            Self::Contour(Contour::new(geometry, columns))
        } else {
            Self::Points
        })
    }

    /// Whether pixels gather nearest samples through a column plan.
    pub(super) fn nearest(&self) -> bool {
        matches!(self, Self::Separable(plan) if plan.nearest())
    }

    /// Bytes retained by the immutable plan.
    pub(super) fn bytes(&self) -> usize {
        match self {
            Self::Separable(plan) => plan.bytes(),
            Self::Points | Self::Contour(_) => 0,
        }
    }

    /// The storage needed for at most `values` values per source sample.
    pub(super) fn layout(&self, values: usize) -> Layout {
        match self {
            Self::Points => Layout::Points,
            Self::Separable(plan) => {
                Layout::Separable(plan.layout(values, separable::Values::Texels))
            }
            Self::Contour(plan) => Layout::Contour(plan.layout(values)),
        }
    }

    /// Writes summed texels of `span` under `geometry`, with exact opaque
    /// alpha and shape wherever a footprint meets the image.
    pub(super) fn fill(
        &self,
        geometry: &Geometry,
        pixels: Pixels<'_>,
        scratch: &mut Scratch,
        texels: &impl Texels,
        out: &mut [f64],
    ) {
        let (plan, scratch) = match (self, scratch) {
            (Self::Separable(plan), Scratch::Separable(scratch)) => (plan, &mut **scratch),
            (Self::Contour(plan), Scratch::Contour(scratch)) => {
                plan.fill(pixels, texels, out, scratch);
                return;
            }
            (Self::Points, Scratch::Points) => {
                geometry.fill(pixels, texels, out);
                return;
            }
            _ => {
                debug_assert!(false, "sampling scratch matches its prepared plan");
                return;
            }
        };
        let lanes = texels.lanes();
        let opaque = texels.opaque();
        let rows = TexelRows {
            texels,
            width: geometry.size().0,
            values: lanes - if opaque { 2 } else { 0 },
        };
        let n = rows.values;
        if plan.nearest() {
            plan.gather(&rows, pixels, scratch, |pixels| {
                for (pixel, source) in out
                    .chunks_exact_mut(lanes)
                    .zip(pixels.chunks_exact(n.max(1)))
                {
                    if let Some((values, rest)) = pixel.split_at_mut_checked(n) {
                        values.copy_from_slice(source);
                        rest.fill(1.0);
                    }
                }
            });
            return;
        }
        out.fill(0.0);
        let finish = |out: &mut [f64], covered: &[bool]| {
            if lanes > n {
                for (pixel, &covered) in out.chunks_exact_mut(lanes).zip(covered) {
                    if let Some(rest) = pixel.get_mut(n..) {
                        rest.fill(f64::from(u8::from(covered)));
                    }
                }
            }
        };
        plan.average(
            &rows,
            pixels,
            scratch,
            Some((out, lanes)),
            (|average| average, finish),
        );
    }
}

/// Rows of converted texels, keeping only colour lanes when opaque.
struct TexelRows<'a, T> {
    texels: &'a T,
    width: u32,
    values: usize,
}

impl<T: Texels> Rows for TexelRows<'_, T> {
    type Value = f64;

    fn width(&self) -> usize {
        self.values
    }

    fn row<'c>(
        &'c self,
        y: usize,
        (first, end): (usize, usize),
        cache: &'c mut RowCache<f64>,
    ) -> &'c [f64] {
        let (Ok(row), Ok(start), Ok(stop)) =
            (u32::try_from(y), u32::try_from(first), u32::try_from(end))
        else {
            return &[];
        };
        cache.get((y, (first, end)), self.values, |out, _| {
            self.texels.row((row, self.width), (start, stop), out);
            true
        })
    }
}
