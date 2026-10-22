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
//! and phase, so the masks the edges keep for their placements, across
//! renders, change time, never pixels. None of this meets the one-level
//! contract.
//!
//! A placement is its mask, offset and bounds ([`GlyphMask`]): a lookup by
//! phase, no geometry. Where its clip state keeps every pixel it covers,
//! its rows are its coverage as they are and store straight into a page;
//! a clip that cuts it is multiplied in through a shape.

use super::coverage::RowOut;
use super::edges::Bounds;
use super::row::Sink;
use super::shape::{Body, Shape};
use super::{ClipShape, DeviceRect, Prep};
use crate::{FillRule, Path, Point, Segment, Transform};
use pagura_core::{Error, Limits, Result};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::{Arc, Mutex};

/// Horizontal origin phases per device pixel.
const PHASES: u16 = 128;

/// The masks one glyph keeps, at most.
const KEPT_MASKS: usize = 1024;

/// The bytes of masks one glyph keeps, at most, unless one mask is larger.
const KEPT_BYTES: usize = 256 << 10;

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
    /// The masks built so far, by phase bits, within `budget` bytes.
    masks: Mutex<Masks>,
    budget: usize,
}

/// A glyph's masks by phases, and their bytes.
#[derive(Debug, Default)]
struct Masks {
    map: HashMap<u32, Built, BuildHasherDefault<PhaseHasher>>,
    bytes: usize,
}

/// One built mask: its cells, rows of the glyph's width, and the columns
/// and rows its coverage lies in, `[x0, y0, x1, y1]`, empty when it has
/// none.
#[derive(Debug, Clone)]
struct Built {
    values: Arc<[u8]>,
    hull: [u32; 4],
}

impl Built {
    fn new(values: Vec<u8>, width: usize) -> Result<Self> {
        let index =
            |i: usize| u32::try_from(i).map_err(|_| Error::limit("glyph extent budget exceeded"));
        let mut hull = [u32::MAX, u32::MAX, 0, 0];
        for (r, row) in values.chunks_exact(width.max(1)).enumerate() {
            if let (Some(first), Some(last)) = (
                row.iter().position(|&v| v != 0),
                row.iter().rposition(|&v| v != 0),
            ) {
                hull = [
                    hull[0].min(index(first)?),
                    hull[1].min(index(r)?),
                    hull[2].max(index(last + 1)?),
                    index(r + 1)?,
                ];
            }
        }
        if hull[0] == u32::MAX {
            hull = [0; 4];
        }
        Ok(Self {
            values: Arc::from(values),
            hull,
        })
    }
}

/// Hashes the one word of a mask's phase key: a multiply, with the high
/// half folded into the bucket bits.
#[derive(Debug, Default)]
struct PhaseHasher(u64);

impl Hasher for PhaseHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.write_u32(u32::from(byte));
        }
    }

    fn write_u32(&mut self, value: u32) {
        self.0 = (self.0 ^ u64::from(value)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }

    fn finish(&self) -> u64 {
        self.0 ^ (self.0 >> 32)
    }
}

impl Prepared {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "validated geometry is normalized before f32 storage and extents are budgeted"
    )]
    pub(crate) fn new(
        outline: &Path,
        linear: [f64; 4],
        scale: f64,
        limits: &Limits,
    ) -> Result<Self> {
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
                masks: Mutex::default(),
                budget: 0,
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
        // Clearing, finishing and looking up each row once.
        let mut work = height.saturating_mul(2 * width + 3);
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
        let cells = width * height;
        Ok(Self {
            resolution,
            edges,
            x,
            y,
            width,
            height,
            work,
            masks: Mutex::default(),
            budget: cells.saturating_mul(KEPT_MASKS).min(KEPT_BYTES).max(cells),
        })
    }

    /// Bytes retained, counting the masks it may keep in full.
    pub(crate) fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            .saturating_add(self.edges.capacity().saturating_mul(size_of::<[f32; 4]>()))
            .saturating_add(self.budget)
            .saturating_add(KEPT_MASKS.saturating_mul(size_of::<(u32, Built)>()))
    }

    fn cells(&self) -> usize {
        self.width.saturating_mul(self.height)
    }

    /// The mask at the phase indices `(px, py)`: kept from an earlier
    /// placement, or built and kept while the glyph's mask bytes allow, all
    /// of them dropped once they would not.
    fn mask(&self, (px, py): (u16, u16)) -> Result<Built> {
        let key = u32::from(px) << 16 | u32::from(py);
        if let Ok(masks) = self.masks.lock()
            && let Some(mask) = masks.map.get(&key)
        {
            return Ok(mask.clone());
        }
        let mut built = Vec::new();
        let phases = (
            f32::from(px) / f32::from(PHASES),
            f32::from(py) / f32::from(self.resolution.vertical_phases()),
        );
        Accumulator::default().build(self, phases, &mut built)?;
        let mask = Built::new(built, self.width)?;
        let cells = mask.values.len();
        if let Ok(mut masks) = self.masks.lock()
            && cells <= self.budget
        {
            if masks.bytes + cells > self.budget || masks.map.len() >= KEPT_MASKS {
                masks.map.clear();
                masks.bytes = 0;
            }
            masks.bytes += cells;
            masks.map.insert(key, mask.clone());
        }
        Ok(mask)
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
    let push = |edges: &mut Vec<(Point, Point)>, edge: (Point, Point)| -> Result<()> {
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
                    [
                        current,
                        snap(c1 * local),
                        snap(c2 * local),
                        snap(end * local),
                    ],
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
/// pixel and given as the index of that phase, and its integer part: whole
/// phase steps, exact for the powers of two that `phases` are.
fn phase(value: f64, phases: u16) -> Result<(u16, i32)> {
    let steps = (value * f64::from(phases)).round();
    if !steps.is_finite() || steps.abs() > 1e8 * f64::from(phases) {
        return Err(Error::limit("glyph origin exceeds working range"));
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a whole number of steps within the range checked above"
    )]
    let steps = steps as i64;
    let phases = i64::from(phases);
    let integer = i32::try_from(steps.div_euclid(phases))
        .map_err(|_| Error::limit("glyph origin exceeds working range"))?;
    let index = u16::try_from(steps.rem_euclid(phases)).unwrap_or(0);
    Ok((index, integer))
}

/// A binned glyph placement whose clip state keeps every pixel its mask
/// can cover: its mask's rows, within its bounds, are its coverage as they
/// are, so they store without passing through band coverage.
#[derive(Debug, Clone)]
pub struct GlyphMask {
    mask: Arc<[u8]>,
    /// The device offset of the mask's first cell.
    x: i64,
    y: i64,
    width: usize,
    /// The pixels it covers within the area, all within the mask.
    bounds: DeviceRect,
    work: usize,
}

impl GlyphMask {
    /// The pixels the mask can cover within the area.
    pub fn bounds(&self) -> DeviceRect {
        self.bounds
    }

    /// Work units emitting its rows costs at most over all its bands, as
    /// [`Shape::work`] bounds a shape's.
    pub fn work(&self) -> usize {
        self.work
    }

    /// Bytes retained: the mask itself is the glyph edges' (ADR 0016).
    pub fn retained_bytes(&self) -> usize {
        size_of::<Self>()
    }

    /// The coverage of row `y` within the bounds: its first column and one
    /// value per pixel, zero where the glyph covers nothing; `None` for a
    /// row outside the bounds.
    pub fn row(&self, y: u32) -> Option<(u32, &[u8])> {
        let b = self.bounds;
        if y < b.y0 || y >= b.y1 {
            return None;
        }
        let row = usize::try_from(i64::from(y) - self.y).ok()?;
        let skip = usize::try_from(i64::from(b.x0) - self.x).ok()?;
        let start = row.checked_mul(self.width)?.checked_add(skip)?;
        let values = self
            .mask
            .get(start..start.checked_add(b.width() as usize)?)?;
        Some((b.x0, values))
    }

    /// The mask's cells within `rect`: the pixels of its bounds there, the
    /// cells from the first of them on, and the distance between rows;
    /// `None` when they meet nowhere.
    pub fn block(&self, rect: DeviceRect) -> Option<(DeviceRect, &[u8], usize)> {
        let block = self.bounds.intersect(&rect);
        if block.is_empty() {
            return None;
        }
        let row = usize::try_from(i64::from(block.y0) - self.y).ok()?;
        let skip = usize::try_from(i64::from(block.x0) - self.x).ok()?;
        let start = row.checked_mul(self.width)?.checked_add(skip)?;
        Some((block, self.mask.get(start..)?, self.width))
    }

    pub(crate) fn add_work(&mut self, units: usize) {
        self.work = self.work.saturating_add(units);
    }
}

/// The glyph masks of one render's placements, by index. Each mask is
/// built where it is kept, and the list keeps its storage when cleared.
#[derive(Debug, Default)]
pub struct GlyphMasks {
    masks: Vec<GlyphMask>,
}

impl GlyphMasks {
    /// Places binned `edges` at `origin` in the clip state `clip` as its
    /// mask when the clip keeps every pixel it covers, and returns the
    /// mask's index: its rows store as they are, without a shape. `None`
    /// for any other placement, which
    /// [`Shape::glyph_with_clip`](super::Shape::glyph_with_clip) prepares,
    /// and for one that covers no pixel of the area.
    ///
    /// # Errors
    ///
    /// As [`Shape::fill`](super::Shape::fill).
    #[inline]
    pub fn place(
        &mut self,
        edges: &super::GlyphEdges,
        origin: Point,
        clip: Option<&ClipShape>,
        options: &super::ShapeOptions<'_>,
    ) -> Result<Option<usize>> {
        let options = Prep::new(options, clip);
        let kept = super::glyph::mask(edges, origin, &options, &mut self.masks)?;
        Ok(kept.then(|| self.masks.len() - 1))
    }

    /// The mask at `index`.
    pub fn get(&self, index: usize) -> Option<&GlyphMask> {
        self.masks.get(index)
    }

    /// The number of masks.
    pub fn len(&self) -> usize {
        self.masks.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.masks.is_empty()
    }

    /// The masks it holds without growing.
    pub fn capacity(&self) -> usize {
        self.masks.capacity()
    }

    /// Drops every mask, keeping the storage.
    pub fn clear(&mut self) {
        self.masks.clear();
    }
}

/// Places binned `prepared` at `origin`: its mask at the origin's phases,
/// kept with the edges or built now, handed with whether its clip state
/// keeps all of it to `keep`; `None` when it covers no pixel of the area.
pub(crate) fn place<R>(
    prepared: &Prepared,
    origin: Point,
    options: &Prep<'_>,
    keep: impl FnOnce(GlyphMask, bool) -> R,
) -> Result<Option<R>> {
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
    // The pixels of the mask's columns and rows `[x0, y0, x1, y1]`.
    let pixels = |[x0, y0, x1, y1]: [i64; 4]| {
        let clamp = |v: i64, lo: u32, hi: u32| {
            u32::try_from(v.clamp(i64::from(lo), i64::from(hi))).unwrap_or(lo)
        };
        DeviceRect {
            x0: clamp(x.saturating_add(x0), limit.x0, limit.x1),
            y0: clamp(y.saturating_add(y0), limit.y0, limit.y1),
            x1: clamp(x.saturating_add(x1), limit.x0, limit.x1),
            y1: clamp(y.saturating_add(y1), limit.y0, limit.y1),
        }
    };
    let width = i64::try_from(prepared.width).unwrap_or(i64::MAX);
    let height = i64::try_from(prepared.height).unwrap_or(i64::MAX);
    if pixels([0, 0, width, height]).is_empty() {
        return Ok(None);
    }
    if prepared.work > options.limits.raster_work {
        return Err(Error::limit(super::shape::RASTER_WORK));
    }
    let built = prepared.mask((px, py))?;
    let hull = built.hull.map(i64::from);
    let bounds = pixels(hull);
    if bounds.is_empty() {
        return Ok(None);
    }
    // Each row is emitted once; a multiplied clip scales each pixel.
    let work = (prepared.width + 3).saturating_mul(bounds.height() as usize);
    if work.saturating_mul(3) > options.limits.raster_work {
        return Err(Error::limit(super::shape::RASTER_WORK));
    }
    let kept = options.clip.is_none_or(|clip| {
        let [x0, y0, x1, y1] = hull;
        let hull = Bounds {
            left: x.saturating_add(x0) as f64,
            top: y.saturating_add(y0) as f64,
            right: x.saturating_add(x1) as f64,
            bottom: y.saturating_add(y1) as f64,
        };
        clip.keeps(hull, bounds)
    });
    let mask = GlyphMask {
        mask: built.values,
        x,
        y,
        width: prepared.width,
        bounds,
        work: if kept { work } else { work * 3 },
    };
    Ok(Some(keep(mask, kept)))
}

/// The shape of a placement prepared in the clip state of `options`: its
/// mask as it is where the clip keeps all of it (`kept`), the clip
/// multiplied in elsewhere.
pub(crate) fn shape(mask: GlyphMask, kept: bool, options: &Prep<'_>) -> Shape {
    let (bounds, work) = (mask.bounds, mask.work);
    let mut shape = Shape::with_body(
        Body::Binned(mask),
        bounds,
        (FillRule::NonZero, work, 0),
        options,
    );
    shape.overlay = false;
    if !kept {
        shape.multiplied = shape.clip.take();
    }
    shape
}

/// One row fragment of an edge, with its area scale.
#[derive(Debug, Clone, Copy)]
struct Fragment {
    left: f32,
    right: f32,
    scale: f32,
    height: f32,
}

/// Signed-area cells of one mask row in f32, with one sentinel.
#[derive(Debug, Default)]
struct Accumulator {
    area: Vec<f32>,
}

impl Accumulator {
    /// The whole mask of `glyph` at `phases` into `mask`, rows of the
    /// glyph's width.
    fn build(&mut self, glyph: &Prepared, phases: (f32, f32), mask: &mut Vec<u8>) -> Result<()> {
        let cells = glyph.cells();
        mask.clear();
        mask.try_reserve(cells)
            .and_then(|()| self.area.try_reserve(cells.saturating_add(glyph.height)))
            .map_err(|_| Error::limit("glyph coverage allocation failed"))?;
        mask.resize(cells, 0);
        if glyph.width > 0 {
            self.mask(glyph, phases, mask);
        }
        Ok(())
    }

    /// The whole mask of `glyph` at phases `(px, py)` into `out`, edge by
    /// edge: each edge's slope once, its fragments row by row, every cell
    /// adding the edges in the order a row alone adds them. Returns the
    /// work spent.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "coverage is clamped to [0, 255] before positive nearest rounding"
    )]
    fn mask(&mut self, glyph: &Prepared, (px, py): (f32, f32), out: &mut [u8]) -> usize {
        let stride = glyph.width + 1;
        let cells = glyph.height * stride;
        self.area.clear();
        self.area.resize(cells, 0.0);
        let mut work = 2 * cells;
        for &edge in &glyph.edges {
            work += 1;
            edge_rows(edge, py, glyph.height, |row, f| {
                let cells = &mut self.area[row * stride..(row + 1) * stride];
                work += 1 + add(cells, f.left + px, f.right + px, f.height, f.scale);
            });
        }
        let level = |value: f32| (value.abs().min(1.0) * 255.0 + 0.5) as u8;
        for (row, line) in self
            .area
            .chunks_exact(stride)
            .zip(out.chunks_exact_mut(glyph.width))
        {
            let mut value = 0.0_f32;
            for (level_out, &delta) in line.iter_mut().zip(row) {
                value += delta;
                *level_out = level(value);
            }
        }
        work
    }
}

/// Adds one fragment's signed area to a row of cells with one sentinel.
/// Returns the cells visited.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "finite fragment ends saturate to the checked local extent"
)]
fn add(row: &mut [f32], left: f32, right: f32, height: f32, scale: f32) -> usize {
    {
        let width = row.len() - 1;
        // Truncation is the floor here: the left end is not negative.
        let cell = left as usize;
        let next = cell as f32 + 1.0;
        if left >= 0.0 && cell < width && right <= next {
            #[expect(clippy::manual_midpoint, reason = "bounded ends; one rounding")]
            let value = (next - (left + right) * 0.5) * height;
            row[cell] += value;
            row[cell + 1] += height - value;
            return 2;
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
        hi.saturating_sub(lo) + 1
    }
}

/// The fragments of edge `[ax, ay, bx, by]` at vertical phase `py`, row by
/// row within a glyph `height` rows high, as [`fragment`] gives each.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "normalized finite geometry and checked extents bound row indices"
)]
fn edge_rows(
    [ax, ay, bx, by]: [f32; 4],
    py: f32,
    height: usize,
    mut store: impl FnMut(usize, Fragment),
) {
    let (mut ax, mut bx) = (ax, bx);
    let (mut ay, mut by) = (ay + py, by + py);
    let sign = if ay < by {
        1.0
    } else {
        std::mem::swap(&mut ax, &mut bx);
        std::mem::swap(&mut ay, &mut by);
        -1.0
    };
    if ay == by {
        return;
    }
    let slope = (bx - ax) / (by - ay);
    if !slope.is_finite() {
        return;
    }
    for row in ay as usize..ceil_index(by).min(height) {
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
        store(
            row,
            Fragment {
                left,
                right,
                scale,
                height,
            },
        );
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

/// Writes the rows of `mask` within `rows`, which lie within its shape's
/// bounds, to `out`, multiplying in the clip its shape carries or is
/// rasterized with. Returns the work spent.
pub(crate) fn rasterize(
    mask: &GlyphMask,
    shape: (&Shape, Option<&ClipShape>),
    rows: std::ops::Range<u32>,
    (out, buffer): (&mut dyn RowOut, &mut Vec<u8>),
) -> usize {
    if shape.0.multiplied.is_none() && shape.1.is_none() {
        return emit(mask, rows, out);
    }
    let width = mask.width;
    let mut work = 1;
    for y in rows {
        let Some(line) = usize::try_from(i64::from(y) - mask.y)
            .ok()
            .and_then(|r| mask.mask.get(r * width..(r + 1) * width))
        else {
            continue;
        };
        out.push_row(y);
        let mut sink = super::raster::sink(out, shape, y, buffer);
        for (x, &value) in (mask.x..).zip(line) {
            if let Ok(x) = usize::try_from(x) {
                sink.run(x, x + 1, value);
            }
        }
        work += width + 3 + sink.extra();
    }
    work
}

/// Writes the rows of `mask` within `rows` to `out` as they are, each as
/// runs at once within the mask's bounds. Returns the work spent.
pub(crate) fn emit(mask: &GlyphMask, rows: std::ops::Range<u32>, out: &mut dyn RowOut) -> usize {
    let mut work = 1;
    for y in rows {
        if let Some((x, values)) = mask.row(y) {
            out.push_row(y);
            out.put_row(x, values);
            work += mask.width + 3;
        }
    }
    work
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PathBuilder;

    #[test]
    fn phases_carry_across_integer_and_negative_origins() {
        assert_eq!(phase(3.999, 128).unwrap(), (0, 4));
        assert_eq!(phase(-0.25, 128).unwrap(), (96, -1));
        assert_eq!(phase(0.97, 16).unwrap(), (0, 1));
        assert_eq!(phase(0.1, 16).unwrap(), (2, 0));
        assert!(phase(f64::INFINITY, 128).is_err());
    }

    #[test]
    fn rectangle_masks_have_rational_areas_at_every_phase() {
        let mut path = PathBuilder::new();
        path.rect(0.0, 0.0, 1.0, 1.0);
        let glyph = Prepared::new(
            &path.finish(),
            [1.0, 0.0, 0.0, 1.0],
            1.0,
            &Limits::default(),
        )
        .unwrap();
        let mut accumulator = Accumulator::default();
        let mut mask = Vec::new();
        for py in [0_u16, 1, 16, 127] {
            for px in [0_u16, 1, 16, 127] {
                accumulator
                    .build(
                        &glyph,
                        (f32::from(px) / 128.0, f32::from(py) / 128.0),
                        &mut mask,
                    )
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
