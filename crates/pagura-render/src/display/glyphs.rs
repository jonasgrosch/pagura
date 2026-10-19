//! Glyph runs: placements of shared outlines and their device geometry cache (9.3.8).

use super::PathCommand;
use pagura_core::{Error, Limits, Result};
use pagura_raster::{Path, Rasterizer, Transform};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    outline: usize,
    linear: [u64; 4],
}

#[derive(Debug)]
pub(crate) struct GlyphSource {
    pub(crate) outline: Arc<Path>,
    pub(crate) linear: Transform,
    hull: [pagura_raster::Point; 2],
    outline_hull: [pagura_raster::Point; 2],
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Placement {
    pub(crate) source: u32,
    pub(crate) origin: pagura_raster::Point,
}

/// Every placement remains a separate mark, including overlapping pixels (9.3.8).
#[derive(Debug)]
pub(crate) struct GlyphRun {
    pub(crate) paint: PathCommand,
    pub(crate) sources: Vec<GlyphSource>,
    pub(crate) glyphs: Vec<Placement>,
    pub(crate) positions: Vec<(usize, pagura_core::Position)>,
    pub(crate) bounds: Envelope,
    origins: Envelope,
    index: std::collections::HashMap<GlyphKey, u32>,
}

/// A running envelope of glyph points in user space. Unlike a [`Rect`] it
/// starts empty, inverted at infinity, and grows with each placement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Envelope {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Envelope {
    const EMPTY: Self = Self {
        x0: f64::INFINITY,
        y0: f64::INFINITY,
        x1: f64::NEG_INFINITY,
        y1: f64::NEG_INFINITY,
    };

    fn include(&mut self, lo: pagura_raster::Point, hi: pagura_raster::Point) {
        self.x0 = self.x0.min(lo.x);
        self.y0 = self.y0.min(lo.y);
        self.x1 = self.x1.max(hi.x);
        self.y1 = self.y1.max(hi.y);
    }

    pub(crate) const fn x0(&self) -> f64 {
        self.x0
    }

    pub(crate) const fn y0(&self) -> f64 {
        self.y0
    }

    pub(crate) const fn x1(&self) -> f64 {
        self.x1
    }

    pub(crate) const fn y1(&self) -> f64 {
        self.y1
    }
}

impl GlyphRun {
    /// Conservative capacity charge, including first-allocation minima and the
    /// hash index's estimated bookkeeping (ADR 0012).
    pub(crate) fn placement_bytes() -> usize {
        4 * (size_of::<Placement>()
            + size_of::<(usize, pagura_core::Position)>()
            + size_of::<GlyphSource>()
            + size_of::<GlyphKey>()
            + size_of::<u32>()
            + 64)
    }

    pub(crate) fn new(paint: PathCommand) -> Self {
        Self {
            paint,
            sources: Vec::new(),
            glyphs: Vec::new(),
            positions: Vec::new(),
            bounds: Envelope::EMPTY,
            origins: Envelope::EMPTY,
            index: std::collections::HashMap::new(),
        }
    }
    pub(crate) fn push(
        &mut self,
        outline: Arc<Path>,
        matrix: Transform,
        position: pagura_core::Position,
    ) -> Result<()> {
        let linear = Transform::new(matrix.a, matrix.b, matrix.c, matrix.d, 0.0, 0.0);
        let key = GlyphKey {
            outline: Arc::as_ptr(&outline) as usize,
            linear: [matrix.a, matrix.b, matrix.c, matrix.d].map(f64::to_bits),
        };
        let source = if let Some(&source) = self.index.get(&key) {
            source
        } else {
            let source = u32::try_from(self.sources.len())
                .map_err(|_| Error::limit("glyph source count exceeded"))?;
            let mut lo = pagura_raster::Point::new(f64::INFINITY, f64::INFINITY);
            let mut hi = pagura_raster::Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
            let mut outline_lo = lo;
            let mut outline_hi = hi;
            let mut record = |p: pagura_raster::Point| -> Result<()> {
                outline_lo.x = outline_lo.x.min(p.x);
                outline_lo.y = outline_lo.y.min(p.y);
                outline_hi.x = outline_hi.x.max(p.x);
                outline_hi.y = outline_hi.y.max(p.y);
                let p = p * linear;
                if !p.is_finite() {
                    return Err(Error::syntax("text matrix arithmetic overflow"));
                }
                lo.x = lo.x.min(p.x);
                lo.y = lo.y.min(p.y);
                hi.x = hi.x.max(p.x);
                hi.y = hi.y.max(p.y);
                Ok(())
            };
            for segment in outline.segments() {
                match *segment {
                    pagura_raster::Segment::MoveTo(p) | pagura_raster::Segment::LineTo(p) => {
                        record(p)?;
                    }
                    pagura_raster::Segment::CubicTo(a, b, c) => {
                        record(a)?;
                        record(b)?;
                        record(c)?;
                    }
                    pagura_raster::Segment::Close => {}
                }
            }
            self.sources
                .try_reserve(1)
                .map_err(|_| Error::limit("glyph source allocation failed"))?;
            self.index
                .try_reserve(1)
                .map_err(|_| Error::limit("glyph index allocation failed"))?;
            self.sources.push(GlyphSource {
                outline,
                linear,
                hull: [lo, hi],
                outline_hull: [outline_lo, outline_hi],
            });
            self.index.insert(key, source);
            source
        };
        let placement = Placement {
            source,
            origin: pagura_raster::Point::new(matrix.e, matrix.f),
        };
        let [lo, hi] = self.sources
            [usize::try_from(source).map_err(|_| Error::limit("glyph index exceeds usize"))?]
        .user_hull(placement.origin);
        if !lo.is_finite() || !hi.is_finite() {
            return Err(Error::syntax("path arithmetic overflow"));
        }
        self.origins.include(placement.origin, placement.origin);
        self.bounds.include(lo, hi);
        if self.positions.last().is_none_or(|(_, p)| *p != position) {
            self.positions
                .try_reserve(1)
                .map_err(|_| Error::limit("glyph position allocation failed"))?;
            self.positions.push((self.glyphs.len(), position));
        }
        self.glyphs
            .try_reserve(1)
            .map_err(|_| Error::limit("glyph placement allocation failed"))?;
        self.glyphs.push(placement);
        Ok(())
    }
    pub(crate) fn bounds_work(&self) -> Result<usize> {
        self.sources
            .len()
            .checked_mul(4)
            .and_then(|n| n.checked_add(4))
            .ok_or(Error::limit("glyph bounds work overflow"))
    }
    /// Bound relative geometry and device origins separately. Forming a user
    /// path first can erase small extents beside a large origin (ADR 0013).
    pub(crate) fn device_hull(&self, device: Transform) -> [pagura_raster::Point; 2] {
        let origins = point_hull(
            corners([
                pagura_raster::Point::new(self.origins.x0(), self.origins.y0()),
                pagura_raster::Point::new(self.origins.x1(), self.origins.y1()),
            ])
            .map(|p| p * device),
        );
        let relative = point_hull(self.sources.iter().flat_map(|s| s.relative_hull(device)));
        point_hull([
            pagura_raster::Point::new(relative[0].x + origins[0].x, relative[0].y + origins[0].y),
            pagura_raster::Point::new(relative[1].x + origins[1].x, relative[1].y + origins[1].y),
        ])
    }
    pub(crate) fn position(&self, index: usize) -> pagura_core::Position {
        let count = self.positions.partition_point(|(first, _)| *first <= index);
        count
            .checked_sub(1)
            .and_then(|i| self.positions.get(i))
            .map_or(self.paint.position, |(_, p)| *p)
    }
    pub(crate) fn source(&self, placement: Placement) -> Result<&GlyphSource> {
        self.sources
            .get(
                usize::try_from(placement.source)
                    .map_err(|_| Error::limit("glyph index exceeds usize"))?,
            )
            .ok_or(Error::invalid_argument("invalid glyph source"))
    }
    pub(crate) fn geometry(
        &self,
        placement: Placement,
        device: Transform,
        limits: &Limits,
    ) -> Result<Arc<pagura_raster::GlyphGeometry>> {
        let source = self.source(placement)?;
        let d = Transform::new(device.a, device.b, device.c, device.d, 0.0, 0.0);
        let l = source.linear.then(d);
        // Text matrices are finite in user space; the device scale can still
        // overflow them, which is the document's extent, not a caller error.
        if !l.is_finite() {
            return Err(Error::limit("glyph device transform overflow"));
        }
        Ok(Arc::new(pagura_raster::GlyphGeometry::new(
            &source.outline,
            [l.a, l.b, l.c, l.d],
            limits,
        )?))
    }
}
impl GlyphSource {
    fn user_hull(&self, origin: pagura_raster::Point) -> [pagura_raster::Point; 2] {
        let transform = Transform::translate(origin.x, origin.y);
        self.hull.map(|p| p * transform)
    }
    pub(crate) fn device_hull(
        &self,
        origin: pagura_raster::Point,
        device: Transform,
    ) -> [pagura_raster::Point; 2] {
        let relative = self.relative_hull(device);
        let origin = origin * device;
        point_hull(relative.map(|p| pagura_raster::Point::new(p.x + origin.x, p.y + origin.y)))
    }
    fn relative_hull(&self, device: Transform) -> [pagura_raster::Point; 2] {
        let linear = self.linear.then(Transform::new(
            device.a, device.b, device.c, device.d, 0.0, 0.0,
        ));
        point_hull(corners(self.outline_hull).map(|p| p * linear))
    }
}
fn corners([lo, hi]: [pagura_raster::Point; 2]) -> [pagura_raster::Point; 4] {
    [
        lo,
        hi,
        pagura_raster::Point::new(lo.x, hi.y),
        pagura_raster::Point::new(hi.x, lo.y),
    ]
}
fn point_hull(points: impl IntoIterator<Item = pagura_raster::Point>) -> [pagura_raster::Point; 2] {
    let mut lo = pagura_raster::Point::new(f64::INFINITY, f64::INFINITY);
    let mut hi = pagura_raster::Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in points {
        // Loose box corners can overflow even if all actual source controls
        // remain finite. An unbounded allocation hint leaves validation to the
        // canonical geometry and avoids culling valid correlated controls.
        if p.x.is_nan() || p.y.is_nan() {
            return [
                pagura_raster::Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
                pagura_raster::Point::new(f64::INFINITY, f64::INFINITY),
            ];
        }
        lo.x = lo.x.min(p.x);
        lo.y = lo.y.min(p.y);
        hi.x = hi.x.max(p.x);
        hi.y = hi.y.max(p.y);
    }
    [lo, hi]
}

#[derive(Debug)]
struct CachedGeometry {
    _outline: Arc<Path>,
    geometry: Arc<pagura_raster::GlyphGeometry>,
}

/// Optional per-render working memory; neither output nor work limits use hits.
#[derive(Debug, Default)]
pub(crate) struct GlyphCache {
    entries: std::collections::HashMap<GlyphKey, CachedGeometry>,
    bytes: usize,
    pub(super) maximum: usize,
}
impl GlyphCache {
    pub(crate) fn new(bytes: usize) -> Self {
        Self {
            maximum: (bytes / 16).min(16 * 1024 * 1024),
            ..Self::default()
        }
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        self.bytes
    }
    pub(crate) fn clear(&mut self) {
        let maximum = self.maximum;
        *self = Self {
            maximum,
            ..Self::default()
        };
    }
    pub(crate) fn geometry(
        &mut self,
        run: &GlyphRun,
        placement: Placement,
        device: Transform,
        limits: &Limits,
        raster: &mut Rasterizer,
    ) -> Result<Arc<pagura_raster::GlyphGeometry>> {
        let source = run.source(placement)?;
        let linear = source.linear.then(Transform::new(
            device.a, device.b, device.c, device.d, 0.0, 0.0,
        ));
        let key = GlyphKey {
            outline: Arc::as_ptr(&source.outline) as usize,
            linear: [linear.a, linear.b, linear.c, linear.d].map(f64::to_bits),
        };
        if let Some(entry) = self.entries.get(&key)
            && entry.geometry.retained_bytes() <= limits.decoded_bytes
        {
            return Ok(Arc::clone(&entry.geometry));
        }
        let resident = self.bytes.saturating_add(raster.retained_bytes());
        let budget = Limits {
            decoded_bytes: limits.decoded_bytes.saturating_sub(resident),
            ..*limits
        };
        let geometry = match run.geometry(placement, device, &budget) {
            Err(error) if resident != 0 && error.kind == pagura_core::ErrorKind::Limit => {
                self.clear();
                raster.clear();
                run.geometry(placement, device, limits)?
            }
            result => result?,
        };
        let bytes = geometry
            .retained_bytes()
            .saturating_add(size_of::<GlyphKey>())
            .saturating_add(size_of::<CachedGeometry>())
            .saturating_add(64);
        let maximum = self.maximum.min(
            limits
                .decoded_bytes
                .saturating_sub(geometry.retained_bytes()),
        );
        if bytes > 64 * 1024
            || bytes > maximum
            || geometry.bounds().is_some_and(|bounds| {
                bounds
                    .iter()
                    .any(|p| p.x.abs().max(p.y.abs()) > f64::from(1_u32 << 30))
            })
        {
            return Ok(geometry);
        }
        if self.bytes.saturating_add(bytes) > maximum {
            self.clear();
        }
        let capacity = self.entries.capacity();
        if self.entries.try_reserve(1).is_ok() {
            let index_bytes = (self.entries.capacity() - capacity)
                .saturating_mul(size_of::<GlyphKey>() + size_of::<CachedGeometry>() + 64);
            let projected = self
                .bytes
                .saturating_add(geometry.retained_bytes())
                .saturating_add(16)
                .saturating_add(index_bytes);
            if projected > maximum {
                self.clear();
                return Ok(geometry);
            }
            self.entries.insert(
                key,
                CachedGeometry {
                    _outline: Arc::clone(&source.outline),
                    geometry: Arc::clone(&geometry),
                },
            );
            self.bytes = projected;
        }
        Ok(geometry)
    }
}
