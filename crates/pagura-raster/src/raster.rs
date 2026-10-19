//! Filling, stroking and clipping with exact polygon area coverage.

use crate::coverage::{Geometry, Shape};

mod reuse;
mod spans;
use crate::flatten::validate;
use crate::quantize::multiply;
use crate::{IntRect, Paint, Path, Pixmap, Transform};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Limits, Result};
pub use reuse::Rasterizer;
pub use spans::Span;
use std::sync::Arc;

/// Which points a fill covers (8.5.3.3). Exhaustive: PDF defines exactly
/// these two rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FillRule {
    /// Nonzero winding number rule.
    #[default]
    NonZero,
    /// Even-odd rule.
    EvenOdd,
}

/// Line cap style (8.4.3.3). Exhaustive: Table 53 closes the set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LineCap {
    /// 0: square end at the endpoint.
    #[default]
    Butt,
    /// 1: semicircular end.
    Round,
    /// 2: square end projecting half the line width.
    ProjectingSquare,
}

/// Line join style (8.4.3.4). Exhaustive: Table 54 closes the set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LineJoin {
    /// 0: mitered, subject to the miter limit.
    #[default]
    Miter,
    /// 1: circular arc.
    Round,
    /// 2: bevelled.
    Bevel,
}

/// Stroke parameters in the path's own (user) space; the stroker applies
/// the transform, so non-uniform scaling distorts the pen as PDF requires.
///
/// A plain record built with functional record update from
/// [`Stroke::default`], so it is not `#[non_exhaustive]`. Its fields are
/// checked when a stroke is outlined, not when it is built.
#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    /// Line width; 0 means the thinnest line the device can render.
    pub width: f64,
    /// Cap style.
    pub cap: LineCap,
    /// Join style.
    pub join: LineJoin,
    /// Miter limit, at least 1.
    pub miter_limit: f64,
    /// Dash lengths; empty means solid.
    pub dash_array: Vec<f64>,
    /// Dash phase.
    pub dash_phase: f64,
}

impl Default for Stroke {
    /// PDF's initial graphics state values (Table 51).
    fn default() -> Self {
        Self {
            width: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 10.0,
            dash_array: Vec::new(),
            dash_phase: 0.0,
        }
    }
}

/// A coverage mask, used for clipping: 255 keeps a pixel, 0 removes it.
///
/// A mask covers an [`IntRect`] in global device space and keeps the
/// geometry it was integrated from, so intersections are computed from
/// geometry rather than by multiplying antialiased coverage (8.5.4).
#[derive(Clone, PartialEq, Eq)]
pub struct Mask {
    area: IntRect,
    coverage: Vec<u8>,
    shapes: Vec<Shape>,
    limits: Limits,
}

/// Summarizes instead of dumping coverage data.
impl std::fmt::Debug for Mask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mask")
            .field("area", &self.area)
            .finish_non_exhaustive()
    }
}

impl Mask {
    /// Rasterizes the area `path` covers under `rule` over `area`, a region
    /// of global device space. Geometry and flattening stay in global space,
    /// so a tile's coverage equals the corresponding pixels of a larger mask,
    /// including rounding decisions. A shared path is not copied.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` for non-finite geometry and `Limit` when
    /// the mask exceeds `limits.pixels` or the geometry exceeds the segment,
    /// work or mask-source budgets.
    pub fn from_path(
        area: IntRect,
        path: impl Into<Arc<Path>>,
        transform: Transform,
        rule: FillRule,
        limits: Limits,
    ) -> Result<Self> {
        let path = path.into();
        validate(&path, transform, &limits)?;
        // A certified empty region rasterizes its device hairline outline.
        let (path, transform, rule) =
            match crate::empty_fill::outline(&path, transform, rule, &limits)? {
                Some(outline) => (Arc::new(outline), Transform::IDENTITY, FillRule::NonZero),
                None => (path, transform, rule),
            };
        let shapes = vec![Shape {
            geometry: Geometry::Path(path),
            transform,
            rule,
        }];
        Self::from_shapes(area, shapes, limits)
    }

    fn from_shapes(area: IntRect, shapes: Vec<Shape>, limits: Limits) -> Result<Self> {
        if shapes.len() > limits.mask_sources {
            return Err(Error::limit("mask source budget exceeded"));
        }
        let count = pixel_budget(area, &limits)?;
        let mut coverage = Vec::new();
        coverage
            .try_reserve_exact(count)
            .map_err(|_| Error::limit("mask allocation failed"))?;
        coverage.resize(count, 0);
        crate::coverage::coverage(
            area,
            &shapes,
            &mut coverage,
            &limits,
            &mut crate::coverage::Scratch::default(),
        )?;
        Ok(Self {
            area,
            coverage,
            shapes,
            limits,
        })
    }

    /// Rasterizes only the path bounds intersected with this clip's area.
    /// Geometry is intersected before coverage integration, never by
    /// multiplying two antialiasing estimates. The result keeps its global
    /// device position.
    ///
    /// # Errors
    ///
    /// As [`Self::from_path`].
    pub fn clipped_path(
        &self,
        path: impl Into<Arc<Path>>,
        transform: Transform,
        rule: FillRule,
        limits: Limits,
    ) -> Result<Option<Self>> {
        Rasterizer::default().clipped_path(self, path, transform, rule, limits)
    }

    /// The pixels this mask covers, in global device space.
    pub fn area(&self) -> IntRect {
        self.area
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.area.width()
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.area.height()
    }

    /// Per-pixel coverage, row-major.
    pub fn coverage(&self) -> &[u8] {
        &self.coverage
    }

    /// The nonzero runs of each row, top to bottom and left to right.
    pub fn spans(&self) -> impl Iterator<Item = Span<'_>> {
        spans::rows(&self.coverage, self.area)
    }

    /// Restricts this mask to the area also covered by `other`.
    ///
    /// # Errors
    ///
    /// Returns an invalid-argument error if the masks cover different areas,
    /// or a limit error for excessive combined geometry or intersection work.
    /// Geometry is intersected before integration; fractional coverage is not
    /// multiplied. The mask is unchanged if intersection fails.
    pub fn intersect(&mut self, other: &Self) -> Result<()> {
        if self.area != other.area {
            return Err(Error::invalid_argument("mask sizes differ"));
        }
        self.restrict(other)
    }

    fn restrict(&mut self, other: &Self) -> Result<()> {
        let needed: Vec<_> = other
            .shapes
            .iter()
            .filter(|shape| !shape.covers(self.area))
            .collect();
        if needed.is_empty() {
            return Ok(());
        }
        let count = self
            .shapes
            .len()
            .checked_add(needed.len())
            .filter(|&n| n <= self.limits.mask_sources)
            .ok_or(Error::limit("mask intersection count budget exceeded"))?;
        let mut shapes = Vec::new();
        shapes
            .try_reserve_exact(count)
            .map_err(|_| Error::limit("mask geometry allocation failed"))?;
        shapes.extend(self.shapes.iter().cloned());
        shapes.extend(needed.into_iter().cloned());
        let mut coverage = Vec::new();
        coverage
            .try_reserve_exact(self.coverage.len())
            .map_err(|_| Error::limit("mask allocation failed"))?;
        coverage.resize(self.coverage.len(), 0);
        crate::coverage::coverage(
            self.area,
            &shapes,
            &mut coverage,
            &self.limits,
            &mut crate::coverage::Scratch::default(),
        )?;
        self.coverage = coverage;
        self.shapes = shapes;
        Ok(())
    }
}

/// The pixel count of a mask over `area`, within `limits.pixels`.
fn pixel_budget(area: IntRect, limits: &Limits) -> Result<usize> {
    usize::try_from(area.pixels())
        .ok()
        .filter(|&count| count <= limits.pixels)
        .ok_or(Error::limit("mask pixel budget exceeded"))
}

/// Converts a user-space stroke to a fill outline and its device transform.
///
/// # Errors
/// Returns invalid geometry or exhausted caller budgets.
pub fn stroke_outline(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
) -> Result<(Path, Transform)> {
    crate::stroke::outline(path, transform, stroke, limits)
}

/// Constructs a device-space outline for an empty polygon filling region
/// with nonempty extent (ISO 32000-2, 10.7.4). Its boundary uses the solid
/// one-device-pixel hairline policy of 8.4.3.2, independently of stroke state.
/// Fill the returned outline with [`FillRule::NonZero`]. Point contours retain
/// their floor cells. Returns `None` for point-only paths, ordinary fills or geometry whose
/// emptiness cannot be certified; positive-area slivers are never widened.
/// Requires a retained [`Path::classify_fill`] certificate. The same outline
/// defines painting and clipping (8.5.4, 10.7.4).
///
/// # Errors
/// Returns invalid geometry or exhausted geometry and work budgets.
pub fn empty_fill_outline(
    path: &Path,
    transform: Transform,
    rule: FillRule,
    limits: &Limits,
) -> Result<Option<Path>> {
    crate::empty_fill::outline(path, transform, rule, limits)
}

/// Converts an automatically grid-fitted stroke to a fill outline (10.7.5).
/// The device's fitting policy is documented in the implementation; dashes
/// are measured in the original user space.
///
/// # Errors
/// As [`stroke_outline`]; fitted principal pen diameters must fit the numeric budget.
pub fn adjusted_stroke_outline(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
) -> Result<(Path, Transform)> {
    crate::adjust::outline(path, transform, stroke, limits)
}

fn check_clip(target: &Pixmap, clip: &Mask) -> Result<()> {
    if (clip.width(), clip.height()) != (target.width(), target.height()) {
        return Err(Error::invalid_argument("clip size differs from surface"));
    }
    Ok(())
}

/// Paints `mask`, placed at `offset` in `target`. The mask lies within its
/// clip, which has the target's size ([`Rasterizer::fill_path`]).
pub(super) fn composite(target: &mut Pixmap, mask: &Mask, paint: Paint, offset: (u32, u32)) {
    let Paint::Solid(color) = paint;
    let stride = u32_to_usize(target.width());
    let (x, y) = (u32_to_usize(offset.0), u32_to_usize(offset.1));
    for span in mask.spans() {
        let coverage = span.coverage();
        let column = x + span.column();
        debug_assert!(
            column + coverage.len() <= stride,
            "mask extends beyond target"
        );
        let start = (y + span.row()) * stride + column;
        let Some(pixels) = target
            .data_mut()
            .get_mut(start * 4..(start + coverage.len()) * 4)
        else {
            continue;
        };
        let pixels = pixels.as_chunks_mut::<4>().0;
        if span.is_solid() && color.a == 255 {
            pixels.fill([color.r, color.g, color.b, 255]);
            continue;
        }
        for (pixel, &coverage) in pixels.iter_mut().zip(coverage) {
            let alpha = multiply(color.a, coverage);
            let inverse = 255 - alpha;
            for (channel, source) in pixel.iter_mut().zip([color.r, color.g, color.b, 255]) {
                let value =
                    u16::from(multiply(source, alpha)) + u16::from(multiply(*channel, inverse));
                *channel = u8::try_from(value).unwrap_or(255);
            }
        }
    }
}
