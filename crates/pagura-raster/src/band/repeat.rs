//! Repeated strokes: a stroke whose path, pen and linear map repeat an
//! earlier one's elsewhere reuses the boundary that placement's preparation
//! found, translated, wherever every coverage byte it gives is provably the
//! byte the placement's own preparation would give. One doubtful byte sends
//! the placement through its own preparation, so the bytes are those of
//! preparing every placement.
//!
//! A placement's own preparation integrates its pieces where the winding
//! certificate leaves no doubt, which gives the area of their union U.
//! Elsewhere the row resolver's sides bound a
//! set that differs from U only within 5δ of the pieces, δ the
//! interpolation error of a line at the placement's coordinates: the order
//! of two pieces it gets wrong, at a crossing height it misplaces or misses
//! or as coincident ones, keeps them within 4δ of each other, and a side's
//! ends are within δ of its line. Its cells add two interpolations per part
//! of a piece and the tails of their sums: a piece splits into at most two
//! more parts than the heights its row's sweep stops at, and those heights
//! are its pieces' ends, cuts and crossings, of pairs whose columns
//! overlap.
//!
//! A repeated placement builds its pieces as usual. With the first
//! placement's structure and every vertex within a measured `d` of the
//! first placement's translated, U moves by at most `d` near the pieces.
//! The first placement's boundary, translated with rounding of an ulp per
//! coordinate, is accumulated with measured arithmetic. A byte that
//! quantizes alike across the sum of these bounds, taken over the most of
//! the pieces that can meet one pixel, around that area is the byte of the
//! placement's own preparation.
//!
//! Templates are kept from a stroke's second placement, only where it
//! needed the resolver, within an eighth of the render's byte budget of
//! their own, so that they never decide whether another shape fits.

use super::cells::Cells;
use super::edges::{self, Line, Polygon};
use super::raster;
use super::shape::{Body, Frame, Resolved, Shape};
use super::{Adjustment, Prep, ShapeOptions};
use crate::stroke::Tiling;
use crate::{FillRule, Path, Point, Segment, Stroke, Transform, quantize_unit};
use pagura_core::{Meter, Result};

/// The boundaries strokes prepared so far, by their shape up to
/// translation, for the rest of one preparation.
///
/// Every byte it keeps, and every byte it works with while it lives, is
/// charged to its own budget before it is allocated: an eighth of the
/// render's, so that it never decides whether another shape fits. A stroke
/// the budget cannot admit is prepared as usual.
#[derive(Debug, Default)]
pub struct StrokeTemplates {
    meter: Option<Meter>,
    table: Table,
    entries: Vec<Entry>,
}

/// What strokes of one key hash have left: a template is kept from a
/// stroke's second placement on, when it is likely to repeat again, and
/// only where that placement needed the row resolver: else its own
/// preparation costs about what certifying a reuse would.
#[derive(Debug, Clone, Copy, Default)]
enum Slot {
    #[default]
    Empty,
    /// A first placement.
    Seen(u64),
    /// A second placement that left no template.
    Barren(u64),
    /// The template of `entries[index]`.
    Kept(u64, u32),
}

impl Slot {
    fn hash(self) -> Option<u64> {
        match self {
            Self::Empty => None,
            Self::Seen(hash) | Self::Barren(hash) | Self::Kept(hash, _) => Some(hash),
        }
    }
}

/// Key hashes, open-addressed and at most half full.
#[derive(Debug, Default)]
struct Table {
    slots: Vec<Slot>,
    filled: usize,
}

/// A repeated stroke's key and template.
#[derive(Debug)]
struct Entry {
    /// The path relative to its first point, the stroke's parameters and
    /// the pen's and device map's linear parts, as bits.
    key: Box<[u64]>,
    template: Template,
}

/// One stroke's first placement, relative to its first piece point.
#[derive(Debug)]
struct Template {
    starts: Box<[usize]>,
    pieces: Box<[Point]>,
    /// The lines of its shape outside resolved rows and the sides of those
    /// rows: each part of the boundary once.
    boundary: Box<[Line]>,
    /// The largest coordinate of the placement.
    magnitude: f64,
    /// Found when the template is first reused.
    extent: Option<Extent>,
}

/// A placement's pieces relative to their first point.
struct Pieces {
    starts: Box<[usize]>,
    pieces: Box<[Point]>,
}

/// What can meet one pixel wherever the shape is translated, at most: its
/// piece sides, their lengths and heights within the pixel, summed, and the
/// boundary lines' lengths; and the heights a sweep of one pixel row can
/// stop at.
#[derive(Debug, Clone, Copy)]
struct Extent {
    sides: f64,
    length: f64,
    height: f64,
    boundary: f64,
    events: f64,
}

/// Coverage error above which no placement is worth certifying: every
/// pixel the boundary misses must still quantize to zero.
const LARGEST_BOUND: f64 = 1e-4;

impl Shape {
    /// A stroke as [`Shape::stroke_with_clip`] makes it, reusing the shape
    /// of an earlier placement of the same stroke in `templates` where that
    /// gives the same coverage bytes, and keeping this one's for later ones.
    ///
    /// # Errors
    ///
    /// As [`Shape::stroke_with_clip`].
    pub fn stroke_repeated(
        path: &Path,
        stroke: &Stroke,
        (pen, transform): (Transform, Transform),
        adjustment: Adjustment,
        clip: Option<&super::ClipShape>,
        options: &ShapeOptions<'_>,
        templates: &mut StrokeTemplates,
    ) -> Result<Option<Self>> {
        let prep = Prep::new(options, clip);
        let (polygon, tiling, spine, mut bytes) =
            Self::stroke_pieces(path, stroke, (pen, transform), adjustment, &prep)?;
        // Only unproven pieces may need the resolver.
        if adjustment != Adjustment::None
            || tiling != Tiling::Unproven
            || polygon.points.is_empty()
            || polygon.points_cells
        {
            return Self::stroke_finish((polygon, tiling, spine), &prep, &mut bytes);
        }
        let key = key(path, stroke, (pen, transform));
        let hash = hash(key.clone());
        let StrokeTemplates {
            meter,
            table,
            entries,
        } = templates;
        let meter = meter.get_or_insert_with(|| {
            Meter::new(
                options.limits.render_bytes / 8,
                "stroke template budget exceeded",
            )
        });
        // Only a shape its clip leaves untouched keeps its own geometry.
        match table.find(hash).map(|i| (i, table.slots[i])) {
            Some((_, Slot::Kept(_, index))) => {
                let entry = &mut entries[index as usize];
                if entry.key.iter().copied().eq(key) && unclipped(&polygon, &prep) {
                    // A reuse that fails, even on a budget, leaves the
                    // meters as it found them.
                    let before = bytes.used();
                    if let Ok(Some(shape)) =
                        reuse(&mut entry.template, &polygon, &prep, &mut bytes, meter)
                    {
                        // A reused boundary needs no spine.
                        bytes.refund(spine.as_ref().map_or(0, crate::stroke::Spine::heap_bytes));
                        return Ok(Some(shape));
                    }
                    bytes.refund(bytes.used().saturating_sub(before));
                }
                Self::stroke_finish((polygon, tiling, spine), &prep, &mut bytes)
            }
            Some((i, Slot::Seen(_))) if unclipped(&polygon, &prep) => {
                let origin = polygon.points[0];
                let copies = polygon.starts.len() * size_of::<usize>()
                    + polygon.points.len() * size_of::<Point>();
                let admitted = meter.charge(copies).is_ok();
                let pieces = admitted.then(|| Pieces {
                    starts: polygon.starts.as_slice().into(),
                    pieces: polygon.points.iter().map(|&p| p - origin).collect(),
                });
                let shape = Self::stroke_finish((polygon, tiling, spine), &prep, &mut bytes);
                let template = match (&shape, pieces) {
                    (Ok(Some(shape)), Some(pieces)) if !shape.resolved.is_empty() => {
                        Template::new(pieces, origin, shape, meter)
                    }
                    _ => None,
                };
                if admitted && template.is_none() {
                    meter.refund(copies);
                }
                let kept = template.and_then(|t| keep(entries, key, t, meter));
                table.slots[i] = match kept {
                    Some(index) => Slot::Kept(hash, index),
                    None => Slot::Barren(hash),
                };
                shape
            }
            Some((_, Slot::Empty)) | None => {
                // A table the budget cannot grow records no more strokes.
                let _ = table.insert(hash, meter);
                Self::stroke_finish((polygon, tiling, spine), &prep, &mut bytes)
            }
            Some((_, Slot::Seen(_) | Slot::Barren(_))) => {
                Self::stroke_finish((polygon, tiling, spine), &prep, &mut bytes)
            }
        }
    }
}

/// Keeps `template` with stroke `key` in `entries`, its index there; `None`,
/// with the template's bytes returned to `meter`, where the budget cannot
/// admit the key and entry.
fn keep(
    entries: &mut Vec<Entry>,
    key: impl Iterator<Item = u64> + Clone,
    template: Template,
    meter: &mut Meter,
) -> Option<u32> {
    let index = u32::try_from(entries.len()).ok();
    let words = key.clone().count();
    let mut stored = Vec::new();
    let admitted = index.is_some()
        && meter.reserve_capacity(entries, 1).is_ok()
        && meter.reserve(&mut stored, words).is_ok();
    if !admitted {
        meter.refund(template.heap_bytes());
        return None;
    }
    stored.extend(key);
    entries.push(Entry {
        key: stored.into_boxed_slice(),
        template,
    });
    index
}

impl Table {
    /// The slot holding `hash`, else the empty one it would take; `None`
    /// before the first slot is made.
    fn find(&self, hash: u64) -> Option<usize> {
        let mask = self.slots.len().checked_sub(1)?;
        #[expect(clippy::cast_possible_truncation, reason = "the low bits of a hash")]
        let mut i = hash as usize & mask;
        // At most half full, so an empty slot ends every search.
        loop {
            match self.slots[i].hash() {
                Some(h) if h != hash => i = (i + 1) & mask,
                _ => return Some(i),
            }
        }
    }

    /// Records a first placement of `hash`, which the table does not hold
    /// yet, doubling the slots first where they would be more than half
    /// full.
    fn insert(&mut self, hash: u64, meter: &mut Meter) -> Result<()> {
        if (self.filled + 1) * 2 > self.slots.len() {
            self.grow(meter)?;
        }
        let i = self.find(hash).unwrap_or_default();
        self.slots[i] = Slot::Seen(hash);
        self.filled += 1;
        Ok(())
    }

    /// Doubles the slots, with the old and new ones charged while both live.
    fn grow(&mut self, meter: &mut Meter) -> Result<()> {
        let len = (self.slots.len() * 2).max(16);
        let mut slots = Vec::new();
        meter.reserve(&mut slots, len)?;
        slots.resize(len, Slot::Empty);
        let old = std::mem::replace(&mut self.slots, slots);
        for &slot in &old {
            if let Some(hash) = slot.hash() {
                let i = self.find(hash).unwrap_or_default();
                self.slots[i] = slot;
            }
        }
        meter.refund(old.len() * size_of::<Slot>());
        Ok(())
    }
}

/// Whether the clip leaves the pieces untouched: no clip rectangle or path
/// meets them and their pixels are all within it.
fn unclipped(polygon: &Polygon, prep: &Prep<'_>) -> bool {
    let Some(hull) = polygon.bounds() else {
        return false;
    };
    let Some(mut frame) = Frame::new(prep) else {
        return false;
    };
    frame.fit(hull);
    let pixels = edges::pixels(hull, prep.area.region, false);
    frame.rect.is_none() && frame.paths.is_none() && frame.limit.intersect(&pixels) == pixels
}

/// The translated boundary of `template` as this placement's shape, where
/// every coverage byte it gives is certified; `None` where one is not. The
/// shape's bytes go on `bytes`, and the template's first extent on `meter`.
fn reuse(
    template: &mut Template,
    polygon: &Polygon,
    prep: &Prep<'_>,
    bytes: &mut Meter,
    meter: &mut Meter,
) -> Result<Option<Shape>> {
    if polygon.starts != *template.starts || polygon.points.len() != template.pieces.len() {
        return Ok(None);
    }
    let origin = polygon.points[0];
    let mut displaced: f64 = 0.0;
    let mut magnitude = template.magnitude;
    for (&p, &q) in polygon.points.iter().zip(&template.pieces) {
        let d = (p - origin) - q;
        displaced = displaced.max(d.x.abs()).max(d.y.abs());
        magnitude = magnitude.max(p.x.abs()).max(p.y.abs());
    }
    // Relative coordinates and their difference may each round by an ulp
    // of the coordinates.
    let displaced = displaced + 8.0 * f64::EPSILON * magnitude;
    let extent = *template.extent.get_or_insert_with(|| {
        extent(
            &template.starts,
            &template.pieces,
            &template.boundary,
            meter,
        )
    });
    let bound = bound(
        extent,
        (displaced, magnitude),
        f64::from(prep.area.region.width()),
    );
    if bound.is_nan() || bound >= LARGEST_BOUND {
        return Ok(None);
    }
    let mut lines = Vec::new();
    bytes.reserve(&mut lines, template.boundary.len())?;
    lines.extend(template.boundary.iter().map(|l| Line {
        a: l.a + origin,
        b: l.b + origin,
    }));
    let Some(shape) = boundary_shape(lines, prep, bytes)? else {
        return Ok(None);
    };
    let ok = certified(&shape, bound, bytes)?;
    Ok(ok.then_some(shape))
}

/// The shape of boundary `lines`, which count once each: no row resolved.
/// `None` where the clip would touch them.
fn boundary_shape(lines: Vec<Line>, prep: &Prep<'_>, bytes: &mut Meter) -> Result<Option<Shape>> {
    let Some(mut frame) = Frame::new(prep) else {
        return Ok(None);
    };
    let points = || lines.iter().flat_map(|l| [l.a, l.b]);
    let (mut lo, mut hi) = (
        Point::new(f64::INFINITY, f64::INFINITY),
        Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
    );
    for p in points() {
        lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    frame.fit([lo, hi]);
    if frame.rect.is_some() || frame.paths.is_some() {
        return Ok(None);
    }
    let Some(bounds) = frame.bounds(points()) else {
        return Ok(None);
    };
    let width = super::row::width(prep.area.region.x1)?;
    let scan = super::clip::scan_columns(&lines, width);
    Shape::edges(
        (lines, Vec::new()),
        FillRule::NonZero,
        (bounds, scan, Resolved::default()),
        &frame,
        prep,
        bytes,
        None,
    )
    .map(Some)
}

/// Whether every pixel of `shape` quantizes to one byte across `bound`
/// plus the measured arithmetic of its cells around its area.
fn certified(shape: &Shape, bound: f64, bytes: &mut Meter) -> Result<bool> {
    let Body::Edges(body) = &shape.body else {
        return Ok(false);
    };
    let mut cells = Cells::new(body.scan.1.saturating_sub(body.scan.0), bytes)?;
    let mut certain = true;
    raster::inspect(shape, &mut cells, |_, _, _, area, arithmetic| {
        let center = area.clamp(0.0, 1.0);
        let error = bound + arithmetic;
        certain &= area.is_finite()
            && error.is_finite()
            && quantize_unit(center - error) == quantize_unit(center + error);
    })?;
    let released = cells.retained_bytes();
    drop(cells);
    bytes.refund(released);
    Ok(certain)
}

impl Template {
    /// The template of a placement with pieces `pieces` relative to
    /// `origin` and shape `shape`, its boundary charged to `meter`; `None`
    /// for pieces without area or a boundary the budget cannot admit.
    fn new(pieces: Pieces, origin: Point, shape: &Shape, meter: &mut Meter) -> Option<Self> {
        let Pieces { starts, pieces } = pieces;
        // The pieces share one orientation. Twice the area left of their
        // downward sides is negative where crossing a downward side from
        // the left enters winding +1, the resolver's sides' sense.
        let swept: f64 = sides(&starts, &pieces)
            .map(|l| (l.a.x + l.b.x) * (l.b.y - l.a.y))
            .sum();
        if swept == 0.0 || !swept.is_finite() {
            return None;
        }
        let reversed = swept > 0.0;
        let mut count = 0;
        boundary(shape, reversed, |_| count += 1);
        let mut lines = Vec::new();
        meter.reserve(&mut lines, count).ok()?;
        boundary(shape, reversed, |l| {
            lines.push(Line {
                a: l.a - origin,
                b: l.b - origin,
            });
        });
        let boundary = lines.into_boxed_slice();
        let magnitude = pieces
            .iter()
            .chain(boundary.iter().flat_map(|l| [&l.a, &l.b]))
            .map(|&p| p + origin)
            .fold(0.0_f64, |m, p| m.max(p.x.abs()).max(p.y.abs()));
        Some(Self {
            starts,
            pieces,
            boundary,
            magnitude,
            extent: None,
        })
    }

    /// The bytes it keeps beyond its own entry.
    fn heap_bytes(&self) -> usize {
        self.starts.len() * size_of::<usize>()
            + self.pieces.len() * size_of::<Point>()
            + self.boundary.len() * size_of::<Line>()
    }
}

/// The bound on how far the coverage of a placement, with vertices
/// displaced by at most `displaced`, coordinates up to `magnitude` and rows
/// `width` pixels wide, can be from the translated boundary's in any pixel,
/// apart from the measured arithmetic of that boundary.
fn bound(extent: Extent, (displaced, magnitude): (f64, f64), width: f64) -> f64 {
    let eps = f64::EPSILON;
    let Extent {
        sides,
        length,
        height,
        boundary,
        events,
    } = extent;
    // A line's interpolation error at these coordinates, as the cell
    // kernel's plain interpolation bounds it; `Line::x` keeps tighter.
    let delta = 16.0 * eps * (1.0 + 2.0 * magnitude);
    // Each preparation's sides bound its pieces' union up to points within
    // 4δ of the pieces, and within δ more of their ends and cuts.
    let near = 5.0 * delta;
    let resolved = 2.0 * near * length + std::f64::consts::PI * near * near * sides;
    // This placement's cells: two interpolations per part of a piece over
    // its height; at most two sums per part, `2 + events` parts per piece,
    // each partial sum within twice their heights; the fold across a row.
    let cells = 2.0 * delta * height + 4.0 * eps * sides * (2.0 + events) * height + eps * width;
    let moved = 2.0 * displaced * length + std::f64::consts::PI * displaced * displaced * sides;
    // Stored relative to the first placement's origin and translated to
    // this one's, each coordinate rounding by up to an ulp.
    let translated = 12.0 * eps * magnitude * boundary;
    (2.0 * resolved + cells + moved + translated) * (1.0 + 16.0 * eps)
}

/// Emits the lines of `shape` that bound its covered set once, each in the
/// resolver's sense: its lines outside resolved rows, cut where those rows
/// begin and end and reversed where `reversed`, and the sides of the
/// resolved rows.
fn boundary(shape: &Shape, reversed: bool, mut emit: impl FnMut(Line)) {
    let Body::Edges(body) = &shape.body else {
        return;
    };
    let is_resolved = |row: i64| {
        u32::try_from(row).is_ok_and(|r| r < u32::MAX && shape.resolved.mask(&(r..r + 1)) != 0)
    };
    for &line in &body.lines {
        if line.is_horizontal() {
            continue;
        }
        let (lo, hi) = (line.a.y.min(line.b.y), line.a.y.max(line.b.y));
        let mut part = |from: f64, to: f64| {
            let (from, to) = (from.max(lo), to.min(hi));
            if from < to {
                let a = Point::new(line.x(from), from);
                let b = Point::new(line.x(to), to);
                let part = if line.a.y < line.b.y {
                    Line { a, b }
                } else {
                    Line { a: b, b: a }
                };
                emit(if reversed {
                    Line {
                        a: part.b,
                        b: part.a,
                    }
                } else {
                    part
                });
            }
        };
        #[expect(clippy::cast_possible_truncation, reason = "device rows fit i64")]
        let (first, last) = (lo.floor() as i64, hi.ceil() as i64);
        let mut run = None;
        for row in first..last {
            match (is_resolved(row), run) {
                (false, None) => run = Some(row),
                (true, Some(start)) => {
                    #[expect(clippy::cast_precision_loss, reason = "device rows are exact in f64")]
                    part(start as f64, row as f64);
                    run = None;
                }
                _ => {}
            }
        }
        if let Some(start) = run {
            #[expect(clippy::cast_precision_loss, reason = "device rows are exact in f64")]
            part(start as f64, last as f64);
        }
    }
    shape.resolved.sides().iter().copied().for_each(emit);
}

/// The sides of the contours `starts` of `points`.
fn sides<'a>(starts: &'a [usize], points: &'a [Point]) -> impl Iterator<Item = Line> + Clone + 'a {
    starts
        .iter()
        .enumerate()
        .flat_map(move |(contour, &start)| {
            let end = starts.get(contour + 1).copied().unwrap_or(points.len());
            let ring = &points[start..end];
            ring.iter()
                .zip(ring.iter().skip(1).chain(ring.first()))
                .map(|(&a, &b)| Line { a, b })
        })
}

/// How far a placement's sides may stand from the template's: far more
/// than any displacement that certifies.
const MARGIN: f64 = 1e-6;

/// The extent of the sides of contours `starts` of `pieces` and of
/// `boundary` lines within one pixel, wherever it lies; unbounded where
/// `meter` cannot admit the grid it works on. A unit square lies within the
/// two-by-two block of grid squares from the one holding its corner, so
/// each line counts at every grid square whose block its box, grown by the
/// margin, meets, with its length and height up to what the block can hold.
fn extent(starts: &[usize], pieces: &[Point], boundary: &[Line], meter: &mut Meter) -> Extent {
    let unbounded = Extent {
        sides: f64::INFINITY,
        length: f64::INFINITY,
        height: f64::INFINITY,
        boundary: f64::INFINITY,
        events: f64::INFINITY,
    };
    let each_side = || sides(starts, pieces);
    let Some(grid) = Grid::of(each_side().chain(boundary.iter().copied()), pieces.len()) else {
        return unbounded;
    };
    // The counts live beside one other sum at a time, and beside the row
    // steps of `heights`.
    let scratch = (2 * grid.len() + grid.height + 1) * size_of::<f64>();
    if meter.charge(scratch).is_err() {
        return unbounded;
    }
    let block = 2.0 * grid.cell;
    let length = |l: Line| (l.b - l.a).length().min(std::f64::consts::SQRT_2 * block);
    let most = |sums: Vec<f64>| sums.into_iter().fold(0.0, f64::max);
    let counts = grid.sums(each_side(), |_| 1.0);
    let extent = Extent {
        sides: counts.iter().copied().fold(0.0, f64::max),
        length: most(grid.sums(each_side(), length)),
        height: most(grid.sums(each_side(), |l| (l.b.y - l.a.y).abs().min(block))),
        boundary: most(grid.sums(boundary.iter().copied(), length)),
        events: heights(each_side(), &grid, &counts),
    };
    meter.refund(scratch);
    extent
}

/// The most heights a row sweep can stop at, wherever the shape is
/// translated: the row's bounds, each side's two ends, a cut per chunk a
/// side crosses (a chunk holds at least 64 pieces), two cuts per range of
/// cells its spine proves covered (at most [`proven_runs`](super::union::proven_runs) of them), and a
/// crossing per pair of sides whose columns overlap. Two such sides meet a
/// common grid square of the row's band, so its squares' `counts` bound the
/// pairs.
fn heights(sides: impl Iterator<Item = Line>, grid: &Grid, counts: &[f64]) -> f64 {
    let stride = grid.width + 1;
    let mut rows = vec![0_i64; grid.height + 1];
    for line in sides {
        let (_, (y0, y1)) = grid.blocks(line);
        rows[y0] += 1;
        rows[y1 + 1] -= 1;
    }
    let mut most = 0.0_f64;
    let mut n = 0;
    for (y, &step) in rows.iter().take(grid.height).enumerate() {
        n += step;
        #[expect(clippy::cast_precision_loss, reason = "a count of sides")]
        let n = n as f64;
        let pairs: f64 = counts[y * stride..y * stride + grid.width]
            .iter()
            .map(|&c| c * (c - 1.0).max(0.0) / 2.0)
            .sum();
        let proven = 1.0 + n / 8.0;
        most = most.max(2.0 + 2.0 * n + n * (1.0 + n / 64.0 + 2.0 * proven) + pairs);
    }
    most
}

/// Squares of side `cell`, a power of two of at least one, over the boxes
/// of some lines, few enough for the lines they serve.
struct Grid {
    left: f64,
    top: f64,
    cell: f64,
    width: usize,
    height: usize,
}

impl Grid {
    fn of(lines: impl Iterator<Item = Line>, count: usize) -> Option<Self> {
        let (mut lo, mut hi) = (
            Point::new(f64::INFINITY, f64::INFINITY),
            Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
        );
        for l in lines {
            for p in [l.a, l.b] {
                lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
                hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
            }
        }
        let size = (hi - lo) + Point::new(4.0, 4.0);
        if !(size.x.is_finite() && size.y.is_finite()) {
            return None;
        }
        // About four squares per line, beside a few for tiny shapes.
        #[expect(clippy::cast_precision_loss, reason = "a rough budget")]
        let budget = 4.0 * count as f64 + 64.0;
        let mut cell = 1.0_f64;
        while size.x / cell * (size.y / cell) > budget {
            cell *= 2.0;
        }
        let left = ((lo.x - MARGIN) / cell).floor() * cell - cell;
        let top = ((lo.y - MARGIN) / cell).floor() * cell - cell;
        let span = |origin: f64, end: f64| {
            let cells = ((end + MARGIN - origin) / cell).floor() + 1.0;
            (cells.is_finite() && (1.0..1e7).contains(&cells)).then(|| {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "bounded above"
                )]
                let cells = cells as usize;
                cells
            })
        };
        Some(Self {
            left,
            top,
            cell,
            width: span(left, hi.x)?,
            height: span(top, hi.y)?,
        })
    }

    /// For each grid square, the sum of `weight` over `lines` whose box,
    /// grown by the margin, meets its two-by-two block; rows of
    /// `width + 1`. Counts and positive weights: the running sums lose at
    /// most rounding against their exact values, which the slack covers.
    fn sums(&self, lines: impl Iterator<Item = Line>, weight: impl Fn(Line) -> f64) -> Vec<f64> {
        let stride = self.width + 1;
        let mut sums = vec![0.0_f64; self.len()];
        for line in lines {
            let w = weight(line);
            let ((x0, x1), (y0, y1)) = self.blocks(line);
            sums[y0 * stride + x0] += w;
            sums[y0 * stride + x1 + 1] -= w;
            sums[(y1 + 1) * stride + x0] -= w;
            sums[(y1 + 1) * stride + x1 + 1] += w;
        }
        for y in 0..=self.height {
            for x in 1..=self.width {
                sums[y * stride + x] += sums[y * stride + x - 1];
            }
        }
        for y in 1..=self.height {
            for x in 0..=self.width {
                sums[y * stride + x] += sums[(y - 1) * stride + x];
            }
        }
        for sum in &mut sums {
            *sum *= 1.0 + 1e-9;
        }
        sums
    }

    /// The number of sums over the grid: one more column and row than it
    /// has squares.
    fn len(&self) -> usize {
        (self.width + 1) * (self.height + 1)
    }

    /// The grid squares, inclusive, whose two-by-two block `line`'s box,
    /// grown by the margin, meets.
    fn blocks(&self, line: Line) -> ((usize, usize), (usize, usize)) {
        let index = |v: f64, origin: f64, cells: usize| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss,
                reason = "clamped to the grid"
            )]
            let i = ((v - origin) / self.cell)
                .floor()
                .clamp(0.0, (cells - 1) as f64) as usize;
            i
        };
        let x = (
            index(
                line.a.x.min(line.b.x) - MARGIN - self.cell,
                self.left,
                self.width,
            ),
            index(line.a.x.max(line.b.x) + MARGIN, self.left, self.width),
        );
        let y = (
            index(
                line.a.y.min(line.b.y) - MARGIN - self.cell,
                self.top,
                self.height,
            ),
            index(line.a.y.max(line.b.y) + MARGIN, self.top, self.height),
        );
        (x, y)
    }
}

/// The repeat key of a stroke, word by word: its path relative to its first
/// point, its parameters and the linear parts of its pen and device map, as
/// bits.
fn key<'a>(
    path: &'a Path,
    stroke: &'a Stroke,
    (pen, transform): (Transform, Transform),
) -> impl Iterator<Item = u64> + Clone + 'a {
    let origin = path
        .segments()
        .iter()
        .find_map(|segment| match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => Some(p),
            Segment::QuadTo { control, .. } => Some(control),
            Segment::CubicTo { c1, .. } => Some(c1),
            Segment::Close => None,
        })
        .unwrap_or_default();
    let relative = move |p: Point| [(p.x - origin.x).to_bits(), (p.y - origin.y).to_bits()];
    let segments = path.segments().iter().flat_map(move |segment| {
        let (tag, points, count) = match *segment {
            Segment::MoveTo(p) => (0, [p; 3], 1),
            Segment::LineTo(p) => (1, [p; 3], 1),
            Segment::QuadTo { control, end } => (2, [control, end, end], 2),
            Segment::CubicTo { c1, c2, end } => (3, [c1, c2, end], 3),
            Segment::Close => (4, [Point::default(); 3], 0),
        };
        let mut words = [tag; 7];
        for (i, &p) in points[..count].iter().enumerate() {
            words[1 + 2 * i..3 + 2 * i].copy_from_slice(&relative(p));
        }
        words.into_iter().take(1 + 2 * count)
    });
    segments
        .chain([
            stroke.width.to_bits(),
            stroke.cap as u64,
            stroke.join as u64,
            stroke.miter_limit.to_bits(),
            stroke.dash_phase.to_bits(),
        ])
        .chain(stroke.dash_array.iter().map(|d| d.to_bits()))
        .chain(
            [
                pen.a,
                pen.b,
                pen.c,
                pen.d,
                transform.a,
                transform.b,
                transform.c,
                transform.d,
            ]
            .map(f64::to_bits),
        )
}

fn hash(key: impl Iterator<Item = u64>) -> u64 {
    key.fold(0, |hash, value| {
        (hash ^ value)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .rotate_left(29)
    })
}

#[cfg(test)]
mod tests;
