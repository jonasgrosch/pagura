//! Shading sources (8.7.4): axial, radial, function-based and mesh
//! shadings evaluated per device pixel with prepared functions and a
//! prepared converter.
//!
//! A pixel takes one sample at its centre, or with `AntiAlias` the box
//! filter of a 2×2 grid at quarter offsets. Samples that miss the shading
//! (outside its domain, `Extend` or mesh) do not count: the pixel's colour
//! is the mean of the samples that hit, and its alpha (which is also its
//! shape) the fraction that hit. A shading pattern's samples outside its
//! `BBox` miss as well, and with a `Background` a missed sample takes the
//! background colour (8.7.4.4). Sampling uses global device coordinates, so
//! results do not depend on bands or regions.
//!
//! Spans avoid repeated work where the result is provably identical: a
//! constant colour is converted once, and an axial shading whose axis is
//! aligned with the device grid reuses one evaluation along a row, or one
//! per column for every row.

mod aligned;
mod colors;
mod geometry;
mod mesh;
#[cfg(test)]
mod tests;

use super::{PaintContext, SpanColors};
use crate::features;
use colors::Colors;
use geometry::{Axial, FunctionDomain, Radial};
use mesh::Mesh;
use pagura_color::{MAX_COMPONENTS, Painted};
use pagura_core::{Error, Meter, Rect, Result};
use pagura_interpret::{Shading, ShadingFunction, ShadingKind};
use pagura_raster::{FillRegion, FillRule, PathBuilder, Point, Transform};

/// The function input of one sample: `t`, `(x, y)` or mesh values.
type Input = [f64; MAX_COMPONENTS];

/// Shading geometry: the function input at a point of shading space.
trait Sampler {
    /// Writes the input at `point` and returns true, or returns false for a
    /// miss.
    fn sample(&self, point: Point, input: &mut Input) -> bool;
}

/// How the executor applies a shading's `BBox`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BBoxUse {
    /// The `BBox` does not limit the painted area.
    None,
    /// The mark is clipped to [`ShadingSource::bbox`] under
    /// [`ShadingSource::transform`] (a degenerate box clips to its outline).
    Clip,
    /// A shading pattern with a `Background` and a degenerate `BBox`: the
    /// executor passes the box outline's coverage to
    /// [`ShadingSource::span_with_boundary`], which mixes shading and
    /// background colours by it.
    Boundary,
}

/// A shading prepared for one render.
#[derive(Debug)]
pub struct ShadingSource {
    /// Shading space to device space, and the `BBox` in shading space.
    frame: Box<(Transform, Option<Rect>)>,
    /// Device space to shading space.
    inverse: Transform,
    /// Samples per pixel along each axis: 1, or 2 with `AntiAlias`.
    rate: u32,
    geometry: Box<Geometry>,
    colors: Box<Colors>,
    pattern: Option<Box<Pattern>>,
    bbox_use: BBoxUse,
    painted: Painted,
    channels: usize,
    /// For a colour that does not depend on the sample: pixel colours by
    /// the number of samples that hit, `rate²` blocks of `channels`.
    constant: Option<Box<[f64]>>,
    /// Whether every sample of every pixel hits.
    covers: bool,
    aligned: Option<Box<aligned::Aligned>>,
}

#[derive(Debug)]
enum Geometry {
    Axial(Axial),
    Radial(Radial),
    Function(FunctionDomain),
    Mesh(Box<Mesh>),
}

/// What a shading pattern adds to `sh` (8.7.4.4).
#[derive(Debug)]
struct Pattern {
    /// The `BBox` samples are tested against; `None` without one or when
    /// it is degenerate (the executor clips to its outline instead).
    bbox: Option<Rect>,
    /// The converted `Background`.
    background: Option<Box<[f64]>>,
}

impl ShadingSource {
    /// Prepares `shading` painted with `sh` under `shading_to_device` (the
    /// CTM composed with the device transform) and reserves the
    /// `function_work` of filling `pixels` device pixels. `Background` is
    /// ignored, as `sh` requires.
    ///
    /// # Errors
    ///
    /// Returns `Unsupported` for a singular transform or colour the
    /// conversion cannot perform, and a `Limit` error when the reservation
    /// exceeds `functions`.
    pub fn prepare(
        shading: &Shading,
        shading_to_device: Transform,
        context: &PaintContext<'_>,
        pixels: usize,
        functions: &mut Meter,
    ) -> Result<Self> {
        Self::new(&Parts::from(shading), shading_to_device, context, Use::Operator)?
            .reserve(pixels, functions)
    }

    /// Prepares `shading` as a shading pattern's paint (8.7.4.4): samples
    /// outside the `BBox` miss, and the `Background` fills missed samples.
    ///
    /// # Errors
    ///
    /// As [`Self::prepare`].
    pub fn pattern(
        shading: &Shading,
        shading_to_device: Transform,
        context: &PaintContext<'_>,
        pixels: usize,
        functions: &mut Meter,
    ) -> Result<Self> {
        Self::new(&Parts::from(shading), shading_to_device, context, Use::Pattern)?
            .reserve(pixels, functions)
    }

    fn new(
        shading: &Parts<'_>,
        transform: Transform,
        context: &PaintContext<'_>,
        usage: Use,
    ) -> Result<Self> {
        let inverse = transform.inverse().ok_or(Error::unsupported(
            features::SHADING_TRANSFORM,
            "singular shading transform",
        ))?;
        let colors = Box::new(Colors::new(shading, context)?);
        let geometry = Box::new(Geometry::new(shading, colors.values())?);
        let certified = shading.bbox.is_some_and(|b| {
            PathBuilder::new()
                .rect(b.x0(), b.y0(), b.width(), b.height())
                .finish()
                .fill_region(FillRule::NonZero)
                .is_some_and(|region| region != FillRegion::Ordinary)
        });
        let pattern = (usage == Use::Pattern).then(|| {
            Box::new(Pattern {
                bbox: shading.bbox.filter(|_| !certified),
                background: shading.background.map(|b| colors.convert_components(b)),
            })
        });
        let bbox_use = match &pattern {
            _ if shading.bbox.is_none() => BBoxUse::None,
            None => BBoxUse::Clip,
            Some(p) if p.background.is_none() => BBoxUse::Clip,
            Some(_) if certified => BBoxUse::Boundary,
            Some(_) => BBoxUse::None,
        };
        let channels = colors.channels();
        let painted = if context.composite.overprint.enabled {
            colors.painted()
        } else {
            Painted::all(channels)
        };
        let rate = if shading.antialias { 2 } else { 1 };
        let mut source = Self {
            frame: Box::new((transform, shading.bbox)),
            inverse,
            rate,
            covers: false,
            geometry,
            colors,
            pattern,
            bbox_use,
            painted,
            channels,
            constant: None,
            aligned: None,
        };
        source.constant = source.constant_colors();
        source.covers = source.always_covers();
        source.aligned = aligned::Aligned::new(&source).map(Box::new);
        Ok(source)
    }

    /// Reserves the function work of `pixels` pixels and of the prepared
    /// tables.
    fn reserve(self, pixels: usize, functions: &mut Meter) -> Result<Self> {
        let samples = pixels
            .checked_mul(self.samples())
            .ok_or(Error::limit("shading sample count overflow"))?;
        let prepared = self.aligned.as_ref().map_or(0, |a| a.pixels());
        let samples = samples
            .checked_add(prepared.saturating_mul(self.samples()))
            .ok_or(Error::limit("shading sample count overflow"))?;
        functions.charge_product(samples, self.colors.cost())?;
        Ok(self)
    }

    fn samples(&self) -> usize {
        let rate = self.rate as usize;
        rate * rate
    }

    /// Shading space to device space.
    pub fn transform(&self) -> Transform {
        self.frame.0
    }

    /// The `BBox` in shading space, if any.
    pub fn bbox(&self) -> Option<Rect> {
        self.frame.1
    }

    /// How the executor applies [`Self::bbox`].
    pub fn bbox_use(&self) -> BBoxUse {
        self.bbox_use
    }

    /// The channels every pixel paints.
    pub fn painted(&self) -> Painted {
        self.painted
    }

    /// The colour of every pixel, when the shading paints one opaque colour
    /// everywhere (a constant function and geometry that every sample hits).
    pub fn constant(&self) -> Option<&[f64]> {
        let all = self.samples() - 1;
        self.constant
            .as_deref()
            .filter(|_| self.covers)
            .and_then(|c| c.get(all * self.channels..(all + 1) * self.channels))
    }

    /// Geometry work units one pixel costs at most, which the executor
    /// reserves as render work: mesh primitive tests (one per triangle, at
    /// most `MAX_WORK` per patch) for each sample; zero for analytic
    /// shadings.
    pub fn geometry_work(&self) -> usize {
        match &*self.geometry {
            Geometry::Mesh(mesh) => mesh.work().saturating_mul(self.samples()),
            _ => 0,
        }
    }

    /// Bytes this source holds besides its function and colour data: mesh
    /// geometry and aligned tables.
    pub fn bytes(&self) -> usize {
        let mesh = match &*self.geometry {
            Geometry::Mesh(mesh) => mesh.bytes(),
            _ => 0,
        };
        mesh + self.aligned.as_ref().map_or(0, |a| a.bytes())
    }

    /// Fills `out` with the colours of device pixels `x .. x + len` of row
    /// `y`. `out.alpha` holds each pixel's covered fraction (its alpha and
    /// shape), or is empty when every pixel is fully covered.
    pub fn span(&self, x: u32, y: u32, len: usize, out: &mut SpanColors) {
        self.fill(x, y, len, None, out);
    }

    /// As [`Self::span`], for [`BBoxUse::Boundary`]: `boundary` holds the
    /// coverage of the box outline at each pixel.
    pub fn span_with_boundary(&self, x: u32, y: u32, boundary: &[u8], out: &mut SpanColors) {
        self.fill(x, y, boundary.len(), Some(boundary), out);
    }

    fn fill(&self, x: u32, y: u32, len: usize, boundary: Option<&[u8]>, out: &mut SpanColors) {
        out.channels = self.channels;
        out.painted = self.painted;
        out.colors.clear();
        out.colors.resize(len * self.channels, 0.0);
        out.alpha.clear();
        out.alpha.resize(len, 1.0);
        let span = Span {
            x,
            y,
            boundary: boundary.filter(|_| self.bbox_use == BBoxUse::Boundary),
        };
        let aligned = self.aligned.as_ref().filter(|_| span.boundary.is_none());
        match &*self.geometry {
            Geometry::Axial(g) => match aligned {
                Some(aligned) => aligned.fill(self, g, span, out),
                None => self.fill_with(g, span, out),
            },
            Geometry::Radial(g) => self.fill_with(g, span, out),
            Geometry::Function(g) => self.fill_with(g, span, out),
            Geometry::Mesh(g) => self.fill_with(&**g, span, out),
        }
        if out.alpha.iter().all(|&a| a == 1.0) {
            out.alpha.clear();
        }
    }

    /// The general span loop, monomorphized per geometry.
    fn fill_with<S: Sampler>(&self, sampler: &S, span: Span<'_>, out: &mut SpanColors) {
        let SpanColors { colors, alpha, .. } = out;
        for (i, (color, alpha)) in colors
            .chunks_exact_mut(self.channels.max(1))
            .zip(alpha.iter_mut())
            .enumerate()
        {
            let x = span.x.saturating_add(u32::try_from(i).unwrap_or(u32::MAX));
            let boundary = span.boundary.and_then(|b| b.get(i)).map(|&c| f64::from(c) / 255.0);
            *alpha = self.pixel(sampler, (x, span.y), boundary, color);
        }
    }

    /// One pixel's colour into `color`, returning its covered fraction.
    fn pixel<S: Sampler>(
        &self,
        sampler: &S,
        (x, y): (u32, u32),
        boundary: Option<f64>,
        color: &mut [f64],
    ) -> f64 {
        if let Some(constant) = &self.constant {
            return self.constant_pixel(sampler, (x, y), constant, color);
        }
        let divisor = f64::from(self.rate);
        let mut input = [0.0; MAX_COMPONENTS];
        let mut covered = 0_u32;
        let background = self.pattern.as_ref().and_then(|p| p.background.as_deref());
        let mixed = boundary.zip(background);
        let mut sample = [0.0; pagura_color::MAX_CHANNELS];
        let sample = sample.get_mut(..self.channels).unwrap_or_default();
        for sy in 0..self.rate {
            for sx in 0..self.rate {
                let point = Point::new(
                    f64::from(x) + (f64::from(sx) + 0.5) / divisor,
                    f64::from(y) + (f64::from(sy) + 0.5) / divisor,
                ) * self.inverse;
                let inside = self.pattern.as_ref().is_none_or(|p| p.inside(point));
                if inside && sampler.sample(point, &mut input) {
                    self.colors.convert(&input, sample);
                } else if let Some(background) = background {
                    sample.copy_from_slice(background);
                } else {
                    continue;
                }
                if let Some((coverage, background)) = mixed {
                    for ((out, &s), &b) in color.iter_mut().zip(&*sample).zip(background) {
                        *out += coverage * s + (1.0 - coverage) * b;
                    }
                } else {
                    for (out, &s) in color.iter_mut().zip(&*sample) {
                        *out += s;
                    }
                }
                covered += 1;
            }
        }
        if covered == 0 {
            return 0.0;
        }
        let count = f64::from(covered);
        for out in color.iter_mut() {
            *out /= count;
        }
        count / (divisor * divisor)
    }

    /// [`Self::pixel`] for a constant colour: only which samples hit
    /// matters, and the mean of that many equal samples is tabulated.
    fn constant_pixel<S: Sampler>(
        &self,
        sampler: &S,
        (x, y): (u32, u32),
        constant: &[f64],
        color: &mut [f64],
    ) -> f64 {
        let divisor = f64::from(self.rate);
        let mut input = [0.0; MAX_COMPONENTS];
        let mut covered = 0_usize;
        for sy in 0..self.rate {
            for sx in 0..self.rate {
                let point = Point::new(
                    f64::from(x) + (f64::from(sx) + 0.5) / divisor,
                    f64::from(y) + (f64::from(sy) + 0.5) / divisor,
                ) * self.inverse;
                let inside = self.pattern.as_ref().is_none_or(|p| p.inside(point));
                covered += usize::from(inside && sampler.sample(point, &mut input));
            }
        }
        let Some(entry) = covered
            .checked_sub(1)
            .and_then(|k| constant.get(k * self.channels..(k + 1) * self.channels))
        else {
            return 0.0;
        };
        color.copy_from_slice(entry);
        let covered = u32::try_from(covered).unwrap_or(0);
        f64::from(covered) / (divisor * divisor)
    }

    /// Pixel colours by hit count when the shading's colour is constant and
    /// no background mixes in.
    fn constant_colors(&self) -> Option<Box<[f64]>> {
        if self.pattern.as_ref().is_some_and(|p| p.background.is_some()) {
            return None;
        }
        let color = self.colors.constant()?;
        let samples = self.samples();
        let mut table = vec![0.0; samples * self.channels];
        for (k, entry) in table.chunks_exact_mut(self.channels.max(1)).enumerate() {
            for (out, &c) in entry.iter_mut().zip(&*color) {
                let mut sum = 0.0;
                for _ in 0..=k {
                    sum += c;
                }
                *out = sum / f64::from(u32::try_from(k + 1).unwrap_or(1));
            }
        }
        Some(table.into())
    }

    /// Whether every sample of every device pixel hits: an axial shading
    /// extended both ways whose parameter stays finite for device
    /// coordinates below 2³², with no `BBox` test.
    fn always_covers(&self) -> bool {
        let Geometry::Axial(axial) = &*self.geometry else {
            return false;
        };
        self.pattern.as_ref().is_none_or(|p| p.bbox.is_none())
            && axial.extend == [true; 2]
            && self.moderate(axial)
    }
}

/// Whether a shading is painted by `sh` or as a shading pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Use {
    Operator,
    Pattern,
}

/// The parts of a shading that preparation reads.
#[derive(Debug, Clone, Copy)]
struct Parts<'a> {
    space: &'a pagura_color::ColorSpace,
    function: Option<&'a ShadingFunction>,
    kind: &'a ShadingKind,
    bbox: Option<Rect>,
    antialias: bool,
    background: Option<&'a [f64]>,
}

impl<'a> From<&'a Shading> for Parts<'a> {
    fn from(shading: &'a Shading) -> Self {
        Self {
            space: &shading.space,
            function: shading.function.as_ref(),
            kind: &shading.kind,
            bbox: shading.bbox,
            antialias: shading.antialias,
            background: shading.background.as_deref(),
        }
    }
}

/// The pixels of one span.
#[derive(Debug, Clone, Copy)]
struct Span<'a> {
    x: u32,
    y: u32,
    boundary: Option<&'a [u8]>,
}

impl Pattern {
    /// Whether a sample lies in the `BBox` (inclusive).
    fn inside(&self, p: Point) -> bool {
        self.bbox
            .is_none_or(|b| p.x >= b.x0() && p.x <= b.x1() && p.y >= b.y0() && p.y <= b.y1())
    }
}

impl Geometry {
    fn new(shading: &Parts<'_>, values: usize) -> Result<Self> {
        Ok(match shading.kind {
            ShadingKind::Axial {
                start,
                end,
                domain,
                extend,
            } => Self::Axial(Axial::new(*start, *end, *domain, *extend)),
            ShadingKind::Radial {
                start,
                start_radius,
                end,
                end_radius,
                domain,
                extend,
            } => Self::Radial(Radial::new(
                (*start, *start_radius),
                (*end, *end_radius),
                *domain,
                *extend,
            )),
            ShadingKind::Function { domain, matrix } => Self::Function(FunctionDomain {
                inverse: matrix.inverse().ok_or(Error::unsupported(
                    features::SHADING_TRANSFORM,
                    "singular shading Matrix",
                ))?,
                domain: *domain,
            }),
            ShadingKind::Triangles(triangles) => {
                Self::Mesh(Box::new(Mesh::triangles(triangles, values)?))
            }
            ShadingKind::Patches(patches) => Self::Mesh(Box::new(Mesh::patches(patches, values)?)),
            _ => {
                return Err(Error::invalid_argument("unknown shading geometry"));
            }
        })
    }
}
