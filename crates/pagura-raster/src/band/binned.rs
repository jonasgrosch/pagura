//! Glyph masks reused per origin bin: the `glyph_origin_bins` fast
//! parameter.
//!
//! An outline is flattened once per linear map at a tolerance chosen by the
//! device resolution: 0.15-pixel chords below 1.5 device pixels per unit
//! (108 dpi), and from there two area-balanced chords per 1/4-pixel piece.
//! Vertices snap to 1/4096 pixel, so that last-bit differences upstream
//! cannot move a rounding boundary between platforms. A placement's origin
//! snaps to 1/128 pixel horizontally and, vertically, to 1/128 pixel below
//! 108 dpi, 1/16 pixel from 108 dpi and 1/64 pixel from 216 dpi; its mask
//! is the f32 signed-area accumulation of the edges' row fragments at that
//! phase, added in clipping order. A mask is a pure function of its edges
//! and phase, so the cache that keeps masks changes time, never pixels.
//! None of this meets the one-level contract.

use super::row::Sink;
use super::shape::{Body, Shape};
use super::{BandCoverage, DeviceRect, GlyphEdges, ShapeOptions, edges};
use crate::{FillRule, Path, Point, Segment, Transform};
use pagura_core::{Error, Limits, Meter, Result};
use std::sync::Arc;

/// Horizontal origin phases per device pixel.
const PHASES: u16 = 128;

/// The largest mask the rasterizer's cache builds and keeps, in cells; a
/// larger glyph's mask is built when its shape is prepared.
const CACHED_CELLS: usize = 1 << 16;

/// The bytes of masks the rasterizer keeps.
const CACHE_BYTES: usize = 4 << 20;

/// The masks the rasterizer keeps at once.
const CACHE_SLOTS: usize = 4096;

/// Spare cells past a mask's last row, as one-cell fragments of its last
/// column reach.
const SPARE: usize = 3;

/// Flattening and vertical origin lattice for a rendering resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolution {
    /// Below 1.5 device pixels per unit.
    Low,
    /// From 1.5 to below 3 device pixels per unit.
    High,
    /// At least 3 device pixels per unit.
    Fine,
}

impl Resolution {
    fn new(scale: f64) -> Self {
        if scale < 1.5 {
            Self::Low
        } else if scale < 3.0 {
            Self::High
        } else {
            Self::Fine
        }
    }

    fn vertical_phases(self) -> u16 {
        match self {
            Self::Low => 128,
            Self::High => 16,
            Self::Fine => 64,
        }
    }

    fn flattening(self) -> Flattening {
        match self {
            Self::Low => Flattening::Chords(0.15),
            Self::High | Self::Fine => Flattening::Balanced(0.25),
        }
    }
}

/// How curves become edges.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Flattening {
    /// Subdivision until each piece's end point chord lies within this
    /// device tolerance.
    Chords(f64),
    /// The same subdivision, then each piece becomes two chords through a
    /// vertex that keeps the piece's signed area.
    Balanced(f64),
}

/// An outline flattened for binned placements: f32 edges relative to its
/// integer extent.
#[derive(Debug)]
pub(crate) struct Prepared {
    resolution: Resolution,
    edges: Vec<[f32; 4]>,
    x: i32,
    y: i32,
    width: usize,
    height: usize,
    /// An upper bound of the work one mask costs.
    work: usize,
}

impl Prepared {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "validated geometry is normalized before f32 storage and extents are budgeted"
    )]
    pub(crate) fn new(outline: &Path, linear: [f64; 4], scale: f64, limits: &Limits) -> Result<Self> {
        let [a, b, c, d] = linear;
        let transform = Transform::new(a, b, c, d, 0.0, 0.0);
        if !transform.is_finite() || !scale.is_finite() {
            return Err(Error::invalid_argument("glyph linear map is not finite"));
        }
        let resolution = Resolution::new(scale);
        let flattening = resolution.flattening();
        let (mut min, mut max) = bounds(outline, transform, limits)?;
        let flatten_limits = Limits {
            path_segments: limits
                .path_segments
                .min(limits.decoded_bytes / 2 / (2 * size_of::<(Point, Point)>())),
            ..*limits
        };
        let lines = flattened_edges(outline, transform, flattening, &flatten_limits)?;
        if matches!(flattening, Flattening::Balanced(_)) {
            // A balanced vertex may leave the control polygon's bounds.
            min = Point::new(f64::INFINITY, f64::INFINITY);
            max = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
            for p in lines.iter().flat_map(|&(a, b)| [a, b]) {
                if !p.is_finite() || p.x.abs().max(p.y.abs()) > 1e8 {
                    return Err(Error::limit("coverage coordinate range exceeded"));
                }
                min = Point::new(min.x.min(p.x), min.y.min(p.y));
                max = Point::new(max.x.max(p.x), max.y.max(p.y));
            }
        }
        if min.x >= max.x || min.y >= max.y {
            return Ok(Self {
                resolution,
                edges: Vec::new(),
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                work: 0,
            });
        }
        let x = min.x.floor() as i32;
        let y = min.y.floor() as i32;
        // One extra column and row hold the largest phase offset.
        let width = (max.x.ceil() - f64::from(x)) as usize + 1;
        let height = (max.y.ceil() - f64::from(y)) as usize + 1;
        width
            .checked_mul(height)
            .filter(|&n| n <= limits.pixels && n <= limits.raster_work)
            .ok_or(Error::limit("glyph extent budget exceeded"))?;
        let mut edges = Vec::new();
        edges
            .try_reserve_exact(lines.len())
            .map_err(|_| Error::limit("glyph geometry allocation failed"))?;
        let mut work = width.saturating_mul(height + 1).saturating_add(SPARE);
        for (a, b) in &lines {
            let e = [
                (a.x - f64::from(x)) as f32,
                (a.y - f64::from(y)) as f32,
                (b.x - f64::from(x)) as f32,
                (b.y - f64::from(y)) as f32,
            ];
            // Rows crossed at any phase, and cells per row fragment.
            let rows = (e[1] - e[3]).abs().ceil() as usize + 2;
            let cells = (e[0] - e[2]).abs().ceil() as usize + 4;
            work = work.saturating_add(rows.saturating_mul(cells.min(width + 4) + 1));
            edges.push(e);
        }
        Ok(Self {
            resolution,
            edges,
            x,
            y,
            width,
            height,
            work,
        })
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        size_of::<Self>().saturating_add(self.edges.capacity().saturating_mul(size_of::<[f32; 4]>()))
    }

    fn cells(&self) -> usize {
        self.width.saturating_mul(self.height)
    }

    /// Whether the rasterizer's cache builds and keeps this glyph's masks.
    fn cacheable(&self) -> bool {
        self.cells() <= CACHED_CELLS
            && (self.width + 1)
                .saturating_mul(self.height)
                .saturating_add(SPARE)
                <= 2 * CACHED_CELLS
    }
}

fn bounds(path: &Path, transform: Transform, limits: &Limits) -> Result<(Point, Point)> {
    let mut min = Point::new(f64::INFINITY, f64::INFINITY);
    let mut max = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    if path.segments().len() > limits.path_segments {
        return Err(Error::limit("coverage geometry budget exceeded"));
    }
    for segment in path.segments() {
        let points: &[Point] = match segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => std::slice::from_ref(p),
            Segment::CubicTo { c1, c2, end } => &[*c1, *c2, *end],
            Segment::Close => &[],
        };
        for &point in points {
            let p = point * transform;
            if !p.is_finite() || p.x.abs().max(p.y.abs()) > 1e8 {
                return Err(Error::limit("coverage coordinate range exceeded"));
            }
            min = Point::new(min.x.min(p.x), min.y.min(p.y));
            max = Point::new(max.x.max(p.x), max.y.max(p.y));
        }
    }
    Ok((min, max))
}

/// Device points snap to a 1/4096-pixel grid.
fn snap(p: Point) -> Point {
    const GRID: f64 = 4096.0;
    Point::new((p.x * GRID).round() / GRID, (p.y * GRID).round() / GRID)
}

fn flattened_edges(
    path: &Path,
    local: Transform,
    flattening: Flattening,
    limits: &Limits,
) -> Result<Vec<(Point, Point)>> {
    let mut edges = Vec::new();
    let mut current = Point::default();
    let mut first = None;
    let mut push = |edges: &mut Vec<(Point, Point)>, edge: (Point, Point)| -> Result<()> {
        edges
            .try_reserve(1)
            .map_err(|_| Error::limit("glyph geometry allocation failed"))?;
        edges.push(edge);
        Ok(())
    };
    for segment in path.segments() {
        match *segment {
            Segment::MoveTo(p) => {
                if let Some(start) = first {
                    push(&mut edges, (current, start))?;
                }
                current = snap(p * local);
                first = Some(current);
            }
            Segment::LineTo(p) => {
                let end = snap(p * local);
                push(&mut edges, (current, end))?;
                current = end;
            }
            Segment::CubicTo { c1, c2, end } => {
                flatten(
                    [current, snap(c1 * local), snap(c2 * local), snap(end * local)],
                    flattening,
                    0,
                    &mut edges,
                    limits,
                )?;
                current = snap(end * local);
            }
            Segment::Close => {
                if let Some(start) = first {
                    push(&mut edges, (current, start))?;
                    current = start;
                }
            }
        }
        if edges.len() > limits.path_segments {
            return Err(Error::limit("coverage edge budget exceeded"));
        }
    }
    if let Some(start) = first {
        push(&mut edges, (current, start))?;
    }
    if edges.len() > limits.path_segments {
        return Err(Error::limit("coverage edge budget exceeded"));
    }
    Ok(edges)
}

fn flatten(
    p: [Point; 4],
    flattening: Flattening,
    depth: usize,
    edges: &mut Vec<(Point, Point)>,
    limits: &Limits,
) -> Result<()> {
    if edges.len() >= limits.path_segments || depth > limits.depth.min(64) {
        return Err(Error::limit("coverage flattening budget exceeded"));
    }
    let (Flattening::Chords(tolerance) | Flattening::Balanced(tolerance)) = flattening;
    let balanced = matches!(flattening, Flattening::Balanced(_));
    let [a, b, c, d] = p;
    let dx = d.x - a.x;
    let dy = d.y - a.y;
    let len = dx * dx + dy * dy;
    let distance = |p: Point| {
        let cross = (p.x - a.x) * dy - (p.y - a.y) * dx;
        cross * cross
    };
    let flat = if len < 1e-20 {
        [b, c]
            .iter()
            .all(|p| (p.x - a.x).abs().max((p.y - a.y).abs()) <= tolerance)
    } else {
        distance(b).max(distance(c)) <= tolerance * tolerance * len
            && [b, c].iter().all(|p| {
                let t = (p.x - a.x) * dx + (p.y - a.y) * dy;
                t >= 0.0 && t <= len
            })
    };
    // A balanced piece needs a chord to move its vertex across, so a closed
    // loop subdivides until it is a point.
    let flat = flat && (!balanced || len >= 1e-20 || (a == b && a == c && a == d));
    edges
        .try_reserve(2)
        .map_err(|_| Error::limit("glyph geometry allocation failed"))?;
    if flat {
        if balanced && len >= 1e-20 {
            if edges.len().saturating_add(2) > limits.path_segments {
                return Err(Error::limit("coverage flattening budget exceeded"));
            }
            let middle = balanced_vertex(p);
            edges.push((a, middle));
            edges.push((middle, d));
        } else {
            edges.push((a, d));
        }
        return Ok(());
    }
    let mid = |a: Point, b: Point| Point::new(a.x.midpoint(b.x), a.y.midpoint(b.y));
    let ab = mid(a, b);
    let bc = mid(b, c);
    let cd = mid(c, d);
    let abc = mid(ab, bc);
    let bcd = mid(bc, cd);
    let m = mid(abc, bcd);
    flatten([a, ab, abc, m], flattening, depth + 1, edges, limits)?;
    flatten([m, bcd, cd, d], flattening, depth + 1, edges, limits)
}

/// The vertex of two chords from `a` to `d` that enclose the cubic's signed
/// area against the chord: the curve's midpoint moved along the chord's
/// normal until its triangle has that area.
fn balanced_vertex([a, b, c, d]: [Point; 4]) -> Point {
    let relative = |p: Point| Point::new(p.x - a.x, p.y - a.y);
    let (b, c, d) = (relative(b), relative(c), relative(d));
    let cross = |p: Point, q: Point| p.x * q.y - p.y * q.x;
    let area = ((cross(b, c) + cross(b, d)) * 3.0 + cross(c, d) * 6.0) / 20.0;
    let middle = Point::new(
        (3.0 * b.x + 3.0 * c.x + d.x) / 8.0,
        (3.0 * b.y + 3.0 * c.y + d.y) / 8.0,
    );
    let shift = 2.0 * (cross(middle, d) * 0.5 - area) / (d.x * d.x + d.y * d.y);
    Point::new(a.x + middle.x - d.y * shift, a.y + middle.y + d.x * shift)
}

/// Splits a device coordinate into its fraction, snapped to `phases` per
/// pixel, and its integer part.
fn phase(value: f64, phases: u16) -> Result<(f64, i32)> {
    let value = (value * f64::from(phases)).round() / f64::from(phases);
    if !value.is_finite() || value.abs() > 1e8 {
        return Err(Error::limit("glyph origin exceeds working range"));
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "finite coordinate range checked above"
    )]
    let integer = value.floor() as i32;
    Ok((value - f64::from(integer), integer))
}

/// One glyph placement in its bin.
#[derive(Debug)]
pub(crate) struct Placement {
    edges: Arc<GlyphEdges>,
    /// The phases, as their bits for the cache key.
    px: f64,
    py: f64,
    /// The device offset of the mask's first cell.
    x: i64,
    y: i64,
    /// A mask too large for the rasterizer's cache, built at preparation.
    mask: Option<Vec<u8>>,
}

/// Places binned `edges` at `origin`.
pub(crate) fn place(
    edges: &Arc<GlyphEdges>,
    prepared: &Prepared,
    origin: Point,
    options: &ShapeOptions<'_>,
) -> Result<Option<Shape>> {
    if prepared.width == 0 {
        return Ok(None);
    }
    let (px, dx) = phase(origin.x, PHASES)?;
    let (py, dy) = phase(origin.y, prepared.resolution.vertical_phases())?;
    let x = i64::from(prepared.x) + i64::from(dx);
    let y = i64::from(prepared.y) + i64::from(dy);
    let region = options.area.region;
    let limit = options
        .clip
        .map_or(region, |clip| clip.bounds().intersect(&region));
    let clamp = |v: i64, lo: u32, hi: u32| u32::try_from(v.clamp(i64::from(lo), i64::from(hi))).unwrap_or(lo);
    let width = i64::try_from(prepared.width).unwrap_or(i64::MAX);
    let height = i64::try_from(prepared.height).unwrap_or(i64::MAX);
    let bounds = DeviceRect {
        x0: clamp(x, limit.x0, limit.x1),
        y0: clamp(y, limit.y0, limit.y1),
        x1: clamp(x.saturating_add(width), limit.x0, limit.x1),
        y1: clamp(y.saturating_add(height), limit.y0, limit.y1),
    };
    if bounds.is_empty() {
        return Ok(None);
    }
    let mask = if prepared.cacheable() {
        None
    } else {
        let mut accumulator = Accumulator::default();
        let mut mask = Vec::new();
        accumulator.build(prepared, (px, py), &mut mask)?;
        Some(mask)
    };
    let bands = options.area.bands().len().min(bounds.height() as usize + 1);
    let work = prepared
        .work
        .saturating_mul(bands)
        .saturating_add((prepared.width + 2).saturating_mul(bounds.height() as usize * 3));
    if work > options.limits.raster_work {
        return Err(Error::limit(super::shape::RASTER_WORK));
    }
    let retained = mask.as_ref().map_or(0, Vec::capacity);
    let mut shape = Shape::with_body(
        Body::Binned(Placement {
            edges: Arc::clone(edges),
            px,
            py,
            x,
            y,
            mask,
        }),
        bounds,
        (FillRule::NonZero, work, retained),
        options,
    );
    if options.clip.is_some() {
        shape.multiplied = options.clip.cloned();
        shape.clip = None;
    }
    Ok(Some(shape))
}

/// One row fragment of an edge, with its area scale.
#[derive(Debug, Clone, Copy)]
struct Fragment {
    start: usize,
    left: f32,
    right: f32,
    scale: f32,
    height: f32,
}

/// Signed-area cells of one mask in f32, one sentinel per row and
/// [`SPARE`] cells after the last.
#[derive(Debug, Default)]
struct Accumulator {
    area: Vec<f32>,
    width: usize,
}

impl Accumulator {
    /// The mask of `glyph` at phases `(px, py)` into `mask`, rows of the
    /// glyph's width.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "canonical phases lie within [0, 1)"
    )]
    fn build(&mut self, glyph: &Prepared, (px, py): (f64, f64), mask: &mut Vec<u8>) -> Result<()> {
        let stride = glyph.width + 1;
        let cells = glyph
            .height
            .checked_mul(stride)
            .and_then(|n| n.checked_add(SPARE))
            .ok_or(Error::limit("glyph extent budget exceeded"))?;
        self.area.clear();
        self.area
            .try_reserve(cells)
            .and_then(|()| mask.try_reserve(glyph.cells().saturating_sub(mask.len())))
            .map_err(|_| Error::limit("glyph coverage allocation failed"))?;
        self.area.resize(cells, 0.0);
        self.width = glyph.width;
        let px = px as f32;
        clip(glyph, py, |f| {
            self.add(f.start, f.left + px, f.right + px, f.height, f.scale);
        });
        self.finish(glyph, mask);
        Ok(())
    }

    /// Adds one fragment's signed area to the row whose first cell is
    /// `start`.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "finite fragment ends saturate to the checked local extent"
    )]
    fn add(&mut self, start: usize, left: f32, right: f32, height: f32, scale: f32) {
        let width = self.width;
        let Some(row) = self.area.get_mut(start..=start + width) else {
            return;
        };
        // Truncation is the floor here: the left end is not negative.
        let cell = left as usize;
        let next = cell as f32 + 1.0;
        if left >= 0.0 && cell < width && right <= next {
            #[expect(clippy::manual_midpoint, reason = "bounded ends; one rounding")]
            let value = (next - (left + right) * 0.5) * height;
            row[cell] += value;
            row[cell + 1] += height - value;
            return;
        }
        let lo = (left as usize).min(width);
        let hi = ceil_index(right).min(width);
        let mut previous = 0.0;
        for (offset, cell) in row[lo..hi].iter_mut().enumerate() {
            let boundary = (lo + offset + 1) as f32;
            let value = if scale == 0.0 {
                (boundary - left.midpoint(right)).clamp(0.0, 1.0) * height
            } else {
                (primitive(boundary - left) - primitive(boundary - right)) * scale
            };
            *cell += value - previous;
            previous = value;
        }
        row[hi] += height - previous;
    }

    /// The mask's levels in rows of the glyph's width.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "coverage is clamped to [0, 255] before positive nearest rounding"
    )]
    fn finish(&self, glyph: &Prepared, mask: &mut Vec<u8>) {
        let level = |value: f32| (value.abs().min(1.0) * 255.0 + 0.5) as u8;
        let stride = glyph.width + 1;
        mask.clear();
        for row in self.area.chunks_exact(stride).take(glyph.height) {
            let mut value = 0.0_f32;
            for &delta in &row[..glyph.width] {
                value += delta;
                mask.push(level(value));
            }
        }
    }
}

/// The area of a unit ramp left of `value`: 0 below 0, v²/2 up to 1 and
/// v - 1/2 beyond.
fn primitive(value: f32) -> f32 {
    let ramp = value.clamp(0.0, 1.0);
    ramp * ramp * 0.5 + (value - ramp).max(0.0)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "saturating conversions; below 2^24 the truncation is exact"
)]
fn ceil_index(value: f32) -> usize {
    let truncated = value as usize;
    truncated.saturating_add(usize::from((truncated as f32) < value))
}

/// Clips the glyph's edges at vertical phase `py` to rows, passing each
/// fragment to `store` edge by edge and row by row.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "normalized finite geometry and checked extents bound row indices"
)]
fn clip(glyph: &Prepared, py: f64, mut store: impl FnMut(Fragment)) {
    let stride = glyph.width + 1;
    for &[ax, ay, bx, by] in &glyph.edges {
        let (mut ax, mut bx) = (ax, bx);
        let (mut ay, mut by) = (ay + py as f32, by + py as f32);
        let sign = if ay < by {
            1.0
        } else {
            std::mem::swap(&mut ax, &mut bx);
            std::mem::swap(&mut ay, &mut by);
            -1.0
        };
        if ay == by {
            continue;
        }
        let rows = ay as usize..ceil_index(by).min(glyph.height);
        let slope = (bx - ax) / (by - ay);
        if !slope.is_finite() {
            continue;
        }
        for row in rows {
            let low = ay.max(row as f32);
            let high = by.min(row as f32 + 1.0);
            let x0 = ax + slope * (low - ay);
            let x1 = ax + slope * (high - ay);
            let left = x0.min(x1);
            let right = x0.max(x1);
            let height = (high - low) * sign;
            let scale = if right - left < 1e-6 {
                0.0
            } else {
                height / (right - left)
            };
            store(Fragment {
                start: row * stride,
                left,
                right,
                scale,
                height,
            });
        }
    }
}

/// A slot of the mask cache.
#[derive(Debug, Default)]
struct Slot {
    edges: Option<Arc<GlyphEdges>>,
    phases: (u64, u64),
    start: usize,
    len: usize,
}

/// The bounded cache of binned glyph masks, allocated once.
#[derive(Debug, Default)]
pub(crate) struct Cache {
    slots: Vec<Slot>,
    store: Vec<u8>,
    accumulator: Accumulator,
    built: Vec<u8>,
}

impl Cache {
    pub(crate) fn new(bytes: &mut Meter) -> Result<Self> {
        let mut cache = Self::default();
        bytes.reserve(&mut cache.slots, CACHE_SLOTS)?;
        cache.slots.resize_with(CACHE_SLOTS, Slot::default);
        bytes.reserve(&mut cache.store, CACHE_BYTES)?;
        bytes.reserve(&mut cache.accumulator.area, 2 * CACHED_CELLS)?;
        bytes.reserve(&mut cache.built, CACHED_CELLS)?;
        Ok(cache)
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.slots
            .capacity()
            .saturating_mul(size_of::<Slot>())
            .saturating_add(self.store.capacity())
            .saturating_add(self.accumulator.area.capacity() * 4)
            .saturating_add(self.built.capacity())
    }

    /// The mask of `placement`, from the cache or built into it.
    fn mask<'a>(&'a mut self, placement: &'a Placement, prepared: &Prepared) -> (&'a [u8], usize) {
        if let Some(mask) = &placement.mask {
            return (mask, 0);
        }
        let key = (placement.px.to_bits(), placement.py.to_bits());
        let address = Arc::as_ptr(&placement.edges) as usize;
        let mut hash = (address as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ key.0.wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
            ^ key.1.rotate_left(17);
        hash ^= hash >> 29;
        let index = (hash as usize) % CACHE_SLOTS;
        let hit = self.slots[index]
            .edges
            .as_ref()
            .is_some_and(|e| Arc::ptr_eq(e, &placement.edges))
            && self.slots[index].phases == key;
        if hit {
            let slot = &self.slots[index];
            return (&self.store[slot.start..slot.start + slot.len], 0);
        }
        let mut built = std::mem::take(&mut self.built);
        let work = prepared.work;
        if self
            .accumulator
            .build(prepared, (placement.px, placement.py), &mut built)
            .is_err()
        {
            built.clear();
        }
        let len = built.len();
        if self.store.len() + len > self.store.capacity() {
            // A full store starts again; every slot then points nowhere.
            self.store.clear();
            for slot in &mut self.slots {
                slot.edges = None;
            }
        }
        let start = self.store.len();
        self.store.extend_from_slice(&built);
        self.built = built;
        self.slots[index] = Slot {
            edges: Some(Arc::clone(&placement.edges)),
            phases: key,
            start,
            len,
        };
        (&self.store[start..start + len], work)
    }

    /// Writes the rows of `placement` within `rows` to `out`. Returns the
    /// work spent.
    pub(crate) fn rasterize(
        &mut self,
        placement: &Placement,
        shape: &Shape,
        rows: std::ops::Range<u32>,
        (out, buffer): (&mut BandCoverage, &mut Vec<u8>),
    ) -> usize {
        let Some(prepared) = placement.edges.prepared() else {
            return 0;
        };
        let (mask, mut work) = self.mask(placement, prepared);
        let width = prepared.width;
        for y in rows {
            let Some(line) = usize::try_from(i64::from(y) - placement.y)
                .ok()
                .and_then(|r| mask.get(r * width..(r + 1) * width))
            else {
                continue;
            };
            out.push_row(y);
            let mut sink = super::raster::sink(out, shape, y, buffer);
            for (x, &value) in (placement.x..).zip(line) {
                if let Ok(x) = usize::try_from(x) {
                    sink.run(x, x + 1, value);
                }
            }
            work += width + 2 + sink.extra();
        }
        work
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PathBuilder;

    #[test]
    fn phases_carry_across_integer_and_negative_origins() {
        assert_eq!(phase(3.999, 128).unwrap(), (0.0, 4));
        assert_eq!(phase(-0.25, 128).unwrap(), (0.75, -1));
        assert_eq!(phase(0.97, 16).unwrap(), (0.0, 1));
        assert_eq!(phase(0.1, 16).unwrap(), (0.125, 0));
        assert!(phase(f64::INFINITY, 128).is_err());
    }

    #[test]
    fn rectangle_masks_have_rational_areas_at_every_phase() {
        let mut path = PathBuilder::new();
        path.rect(0.0, 0.0, 1.0, 1.0);
        let glyph = Prepared::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], 1.0, &Limits::default()).unwrap();
        let mut accumulator = Accumulator::default();
        let mut mask = Vec::new();
        for py in [0_u16, 1, 16, 127] {
            for px in [0_u16, 1, 16, 127] {
                accumulator
                    .build(&glyph, (f64::from(px) / 128.0, f64::from(py) / 128.0), &mut mask)
                    .unwrap();
                assert_eq!(glyph.width, 2);
                for (index, a, b) in [
                    (0, 128 - px, 128 - py),
                    (1, px, 128 - py),
                    (2, 128 - px, py),
                    (3, px, py),
                ] {
                    assert_eq!(
                        mask[index],
                        u8::try_from((u32::from(a) * u32::from(b) * 255 + 8192) / 16384).unwrap()
                    );
                }
            }
        }
    }
}
