//! One prepared sampling plan for converted texels and mask opacities.
//! Integer code averages share the separable kernel in `separable`.

use super::contour::{self, Contour};
use super::geometry::{BilinearRows, Geometry, Mode, Texels};
use super::scratch::allocate;
use super::separable::{self, RowCache, Rows, Separable};
use crate::source::{Pixels, copy_values};
use pagura_core::{Meter, Result};

/// Exactly one way to sample a placement, selected during preparation.
#[derive(Debug)]
pub(super) enum Sampling {
    /// Nearest or bilinear samples of individual pixels; an upright
    /// bilinear span reads its two source rows once, at most `cells` texels
    /// each.
    Points { cells: usize },
    /// Upright nearest samples or exact area averages.
    Separable(Separable),
    /// Exact area averages of a turned placement.
    Contour(Contour),
}

/// Capacities compared before render storage is reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Layout {
    /// Values of the two rows an upright bilinear span reads.
    Points(usize),
    Separable(separable::Layout),
    Contour(contour::Layout),
}

/// Mutable values owned by the caller, never by a prepared scene.
#[derive(Debug)]
pub(super) enum Scratch {
    Points(BilinearRows),
    Separable(Box<separable::Scratch>),
    Contour(contour::Scratch),
}

impl Layout {
    /// Bytes allocated for this sampling plan.
    pub(super) fn bytes(self) -> usize {
        match self {
            Self::Points(values) => values.saturating_mul(size_of::<f64>()),
            Self::Separable(layout) => layout.bytes(),
            Self::Contour(layout) => layout.bytes(),
        }
    }

    /// Allocates capacities already charged by the source layout.
    pub(super) fn make(self) -> Result<Scratch> {
        Ok(match self {
            Self::Points(values) => Scratch::Points(BilinearRows::new(allocate(values)?)),
            Self::Separable(layout) => Scratch::Separable(Box::new(layout.make()?)),
            Self::Contour(layout) => Scratch::Contour(layout.make()?),
        })
    }
}

impl Scratch {
    /// Actual capacities held for this plan.
    pub(super) fn bytes(&self) -> usize {
        match self {
            Self::Points(rows) => rows.bytes(),
            Self::Separable(scratch) => scratch.bytes(),
            Self::Contour(scratch) => scratch.bytes(),
        }
    }

    /// Invalidates rows before another render can use this storage.
    pub(super) fn reset(&mut self) {
        match self {
            Self::Points(rows) => rows.reset(),
            Self::Separable(scratch) => scratch.reset(),
            Self::Contour(_) => {}
        }
    }
}

impl Sampling {
    /// Plans `geometry` over device `columns`.
    ///
    /// # Errors
    ///
    /// A `Limit` error when the column plan cannot be allocated.
    pub(super) fn new(geometry: &Geometry, columns: (u32, u32), bytes: &mut Meter) -> Result<Self> {
        // Bilinear pixels keep their four products in the same order for
        // every placement, including upright ones.
        if geometry.mode() != Mode::Bilinear
            && let Some(separable) = Separable::new(geometry, columns, bytes)?
        {
            return Ok(Self::Separable(separable));
        }
        Ok(if geometry.mode() == Mode::Area {
            Self::Contour(Contour::new(geometry, columns))
        } else {
            Self::Points {
                cells: geometry.row_cells(columns),
            }
        })
    }

    /// Bytes retained by the immutable plan.
    pub(super) fn bytes(&self) -> usize {
        match self {
            Self::Separable(plan) => plan.bytes(),
            Self::Points { .. } | Self::Contour(_) => 0,
        }
    }

    /// The storage needed for at most `values` values per source sample.
    pub(super) fn layout(&self, values: usize) -> Layout {
        match self {
            Self::Points { cells } => Layout::Points(cells.saturating_mul(2 * values)),
            Self::Separable(plan) => {
                Layout::Separable(plan.layout(values, separable::Values::Texels))
            }
            Self::Contour(plan) => Layout::Contour(plan.layout(values)),
        }
    }

    /// Writes summed texels of `span` under `geometry`, every value of
    /// `out`, with exact opaque alpha and shape wherever a footprint meets
    /// the image.
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
            (Self::Points { .. }, Scratch::Points(rows)) => {
                geometry.fill(pixels, texels, out, rows);
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
                        copy_values(values, source);
                        opaque_lanes(rest, 1.0);
                    }
                }
            });
            return;
        }
        let finish = |out: &mut [f64], covered: &[bool]| {
            if lanes > n {
                for (pixel, &covered) in out.chunks_exact_mut(lanes).zip(covered) {
                    if let Some(rest) = pixel.get_mut(n..) {
                        opaque_lanes(rest, f64::from(u8::from(covered)));
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

impl Sampling {
    /// The words an opaque store writes for the colours of nearest opaque
    /// three-colour texels of `pixels`, written as their rows are read, as
    /// [`Self::fill`] reads them: false, with nothing written, unless the
    /// plan takes upright nearest samples of such texels.
    pub(super) fn words(
        &self,
        geometry: &Geometry,
        pixels: Pixels<'_>,
        scratch: &mut Scratch,
        texels: &impl Texels,
        out: &mut [[u16; 4]],
    ) -> bool {
        let (Self::Separable(plan), Scratch::Separable(scratch)) = (self, scratch) else {
            return false;
        };
        if !plan.nearest() || !texels.opaque() || texels.lanes() != 5 {
            return false;
        }
        let rows = TexelRows {
            texels,
            width: geometry.size().0,
            values: 3,
        };
        let word = crate::kernel::opaque_word;
        plan.gather_each(
            &rows,
            pixels,
            scratch,
            out,
            |pixel, &[r, g, b]: &[f64; 3]| {
                *pixel = [word(r), word(g), word(b), u16::MAX];
            },
        );
        true
    }
}

/// Sets the alpha and shape that follow an opaque texel's colours: two
/// stores rather than a fill call per pixel.
#[inline]
fn opaque_lanes(rest: &mut [f64], value: f64) {
    match rest {
        [alpha, shape] => (*alpha, *shape) = (value, value),
        rest => rest.fill(value),
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
