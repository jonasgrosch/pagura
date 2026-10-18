//! Filling, stroking and clipping with exact polygon area coverage.

mod region;
mod reuse;
mod runs;
pub(crate) mod spans;
use crate::quantize::multiply;
use crate::{Paint, Path, Pixmap, Transform};
use pagura_core::{Error, Limits, Result};
pub use region::ClipRegion;
pub use reuse::Rasterizer;
pub use runs::{Coverage, Rectangle};

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

/// The integrated coverage of a [`ClipRegion`]: 255 keeps a pixel, 0 removes it.
#[derive(Clone, PartialEq, Eq)]
pub struct Mask {
    region: ClipRegion,
    coverage: Vec<u8>,
}

/// Summarizes instead of dumping coverage data.
impl std::fmt::Debug for Mask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mask")
            .field("width", &self.region.width)
            .field("height", &self.region.height)
            .finish_non_exhaustive()
    }
}

impl Mask {
    /// Global device origin of this coverage region.
    pub fn origin(&self) -> (u32, u32) {
        self.region.origin
    }

    /// The geometry this mask integrates, as a clip region.
    pub fn region(&self) -> &ClipRegion {
        &self.region
    }

    /// Rasterizes only the path bounds intersected with this clip's surface.
    /// Geometry is intersected before coverage integration, never by multiplying
    /// two antialiasing estimates. The result retains its global device origin.
    ///
    /// # Errors
    /// Returns geometry and caller-budget errors as [`Self::from_path_with_limits`].
    pub fn clipped_path(
        &self,
        path: &Path,
        transform: Transform,
        rule: FillRule,
        limits: Limits,
    ) -> Result<Option<Self>> {
        self.region.clipped_path(path, transform, rule, limits)
    }
    /// Rasterizes the area `path` covers under `rule` into a mask of the
    /// given size.
    ///
    /// # Errors
    ///
    /// Returns a limit error if the mask exceeds `max_pixels` and an
    /// invalid-argument error for non-finite geometry.
    pub fn from_path(
        width: u32,
        height: u32,
        path: &Path,
        transform: Transform,
        rule: FillRule,
        max_pixels: usize,
    ) -> Result<Self> {
        Self::from_path_at(width, height, path, transform, rule, max_pixels, (0, 0))
    }

    /// Rasterizes a tile whose integer origin is in global device coordinates.
    /// Geometry and flattening stay in global space, making tile areas identical
    /// to the corresponding full-surface pixels, including rounding decisions.
    ///
    /// # Errors
    /// As [`Self::from_path`]. Returns `Limit` if the tile extent overflows u32.
    pub fn from_path_at(
        width: u32,
        height: u32,
        path: &Path,
        transform: Transform,
        rule: FillRule,
        max_pixels: usize,
        origin: (u32, u32),
    ) -> Result<Self> {
        Self::from_path_with_limits(
            width,
            height,
            path,
            transform,
            rule,
            Limits {
                pixels: max_pixels,
                ..Limits::default()
            },
            origin,
        )
    }

    /// Rasterizes a path with explicit geometry, work and mask-source budgets.
    ///
    /// # Errors
    /// As [`Self::from_path_at`], with limits from `limits`.
    pub fn from_path_with_limits(
        width: u32,
        height: u32,
        path: &Path,
        transform: Transform,
        rule: FillRule,
        limits: Limits,
        origin: (u32, u32),
    ) -> Result<Self> {
        Self::integrate(ClipRegion::from_path(
            width, height, path, transform, rule, limits, origin,
        )?)
    }

    fn integrate(region: ClipRegion) -> Result<Self> {
        let count = region.pixels()?;
        let mut coverage = Vec::new();
        coverage
            .try_reserve_exact(count)
            .map_err(|_| Error::limit("mask allocation failed"))?;
        coverage.resize(count, 0);
        crate::coverage::coverage(
            region.width,
            region.height,
            &region.shapes,
            &mut coverage,
            region.origin,
            &region.limits,
            &mut crate::coverage::Scratch::default(),
        )?;
        Ok(Self { region, coverage })
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.region.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.region.height
    }

    /// Per-pixel coverage, row-major.
    pub fn coverage(&self) -> &[u8] {
        &self.coverage
    }

    /// Nonzero horizontal runs as `(row, column, coverage)` in mask coordinates.
    /// Each run contains either only 255 or only fractional samples (1..=254).
    /// Runs stay within a row and borrow the bytes of [`Self::coverage`].
    pub fn spans(&self) -> impl Iterator<Item = (usize, usize, &[u8])> {
        // Mask construction checked that its width fits the allocated buffer.
        spans::rows(
            &self.coverage,
            usize::try_from(self.region.width).unwrap_or(usize::MAX),
        )
    }

    /// Restricts this mask to the area also covered by `other`.
    ///
    /// # Errors
    ///
    /// Returns an invalid-argument error if sizes or global origins differ,
    /// or a limit error for excessive combined geometry or intersection work.
    /// Geometry is intersected before integration; fractional coverage is not
    /// multiplied. The mask is unchanged if intersection fails.
    pub fn intersect(&mut self, other: &Self) -> Result<()> {
        let mut region = self.region.clone();
        region.intersect(&other.region)?;
        if region.shapes.len() != self.region.shapes.len() {
            *self = Self::integrate(region)?;
        }
        Ok(())
    }
}

/// Fills the area of `path` (mapped by `transform`) with `paint`,
/// antialiased by exact area coverage, optionally restricted by `clip`.
/// Retained [`Path::classify_fill`] certificates opt into empty-boundary
/// hairlines; unprepared geometry uses ordinary scan conversion.
///
/// # Errors
///
/// Returns an invalid-argument error for non-finite geometry or a clip of
/// a different size, and a limit error if the path exceeds internal budgets.
pub fn fill_path(
    target: &mut Pixmap,
    path: &Path,
    transform: Transform,
    rule: FillRule,
    paint: &Paint,
    clip: Option<&Mask>,
) -> Result<()> {
    fill_path_at(target, path, transform, rule, paint, clip, (0, 0))
}

/// Fills a tile using geometry in global device coordinates.
///
/// # Errors
/// As [`fill_path`]; also rejects overflowing tile extents.
pub fn fill_path_at(
    target: &mut Pixmap,
    path: &Path,
    transform: Transform,
    rule: FillRule,
    paint: &Paint,
    clip: Option<&Mask>,
    origin: (u32, u32),
) -> Result<()> {
    let clip = clip.map(Mask::region);
    check_clip(target, clip)?;
    let limits = *target.limits();
    let Some(mask) = fill_mask(
        path,
        transform,
        rule,
        clip,
        (target.width(), target.height()),
        origin,
        limits,
    )?
    else {
        return Ok(());
    };
    let (x, y) = mask.origin();
    composite(target, &mask, *paint, (x - origin.0, y - origin.1))?;
    Ok(())
}

fn fill_mask(
    path: &Path,
    transform: Transform,
    rule: FillRule,
    clip: Option<&ClipRegion>,
    size: (u32, u32),
    origin: (u32, u32),
    limits: Limits,
) -> Result<Option<Mask>> {
    Rasterizer::default().path_mask(
        reuse::PathSource::Borrowed(path),
        transform,
        rule,
        limits,
        (size, origin, clip),
    )
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

/// [`stroke_outline`] whose retained boundary, when every run admits one, is
/// made of pieces that meet without overlapping, so that signed-area coverage
/// (see [`Rasterizer::with_approximate_cells`]) integrates the stroke without
/// a union. The filled set is the same.
///
/// # Errors
/// As [`stroke_outline`].
pub fn tiled_stroke_outline(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
) -> Result<(Path, Transform)> {
    crate::stroke::tiled_outline(path, transform, stroke, limits)
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

/// Strokes `path` with `stroke` (in the path's space, then mapped by
/// `transform`) and composites `paint`, optionally restricted by `clip`.
///
/// # Errors
///
/// See [`fill_path`].
pub fn stroke_path(
    target: &mut Pixmap,
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    paint: &Paint,
    clip: Option<&Mask>,
) -> Result<()> {
    stroke_path_at(target, path, transform, stroke, paint, clip, (0, 0))
}

/// Strokes a tile using geometry in global device coordinates.
///
/// # Errors
/// As [`stroke_path`]; also rejects overflowing tile extents.
pub fn stroke_path_at(
    target: &mut Pixmap,
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    paint: &Paint,
    clip: Option<&Mask>,
    origin: (u32, u32),
) -> Result<()> {
    check_clip(target, clip.map(Mask::region))?;
    let (outline, transform) = crate::stroke::outline(path, transform, stroke, target.limits())?;
    fill_path_at(
        target,
        &outline,
        transform,
        FillRule::NonZero,
        paint,
        clip,
        origin,
    )
}

/// Applies automatic device grid fitting to a stroke.
/// See ISO 32000-2, 10.7.5; the device's fitting policy is documented in the
/// implementation. Dashes are measured in original user space.
///
/// # Errors
/// As [`stroke_path_at`]; fitting preserves curved/oblique centrelines.
pub fn stroke_path_adjusted_at(
    target: &mut Pixmap,
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    paint: &Paint,
    clip: Option<&Mask>,
    origin: (u32, u32),
) -> Result<()> {
    check_clip(target, clip.map(Mask::region))?;
    let (outline, transform) = crate::adjust::outline(path, transform, stroke, target.limits())?;
    fill_path_at(
        target,
        &outline,
        transform,
        FillRule::NonZero,
        paint,
        clip,
        origin,
    )
}

fn check_clip(target: &Pixmap, clip: Option<&ClipRegion>) -> Result<()> {
    if clip.is_some_and(|clip| (clip.width, clip.height) != (target.width(), target.height())) {
        return Err(Error::invalid_argument("clip size differs from surface"));
    }
    Ok(())
}

pub(super) fn composite(
    target: &mut Pixmap,
    mask: &Mask,
    paint: Paint,
    offset: (u32, u32),
) -> Result<()> {
    let Paint::Solid(color) = paint;
    let stride =
        usize::try_from(target.width()).map_err(|_| Error::limit("surface width exceeds usize"))?;
    let x = usize::try_from(offset.0).map_err(|_| Error::limit("column exceeds usize"))?;
    let y = usize::try_from(offset.1).map_err(|_| Error::limit("row exceeds usize"))?;
    for (row, column, coverage) in mask.spans() {
        if x + column + coverage.len() > stride {
            return Err(Error::invalid_argument("mask extends beyond target"));
        }
        let start = (y + row) * stride + x + column;
        let pixels = target
            .data_mut()
            .get_mut(start * 4..(start + coverage.len()) * 4)
            .ok_or(Error::invalid_argument("mask extends beyond target"))?
            .as_chunks_mut::<4>()
            .0;
        if coverage[0] == 255 && color.a == 255 {
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
    Ok(())
}
