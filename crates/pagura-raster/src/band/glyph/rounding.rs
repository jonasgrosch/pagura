//! Coverage-byte certification at the preparation boundary. A small area
//! error alone cannot survive repeated composition: both ends of the true
//! coverage interval must quantize to the byte execution will emit. Exact
//! quadratics integrated in closed form skip inspection only while a cached
//! arithmetic allowance stays small at their actual placement.
//!
//! A pixel the bound leaves in doubt takes its byte from the original
//! outline, once per placement, together with the other such pixels of its
//! row and from only the outline's items that reach the row. A wrong byte
//! replaces the geometry of its row, not of the whole placement.

use super::row::{Making, Rows};
use super::{GlyphEdges, Kind, Relative, area_share, flattened_with, point_sides, translated};
use crate::band::cells::Cells;
use crate::band::clip::Paths;
use crate::band::edges::{Bounds, Line, rows};
use crate::band::shape::{Body, Frame, GlyphGeometry, Shape, Winding};
use crate::band::union::{Meters, Piece, Resolver};
use crate::band::{DeviceRect, Prep, RASTER_WORK, RENDER_BYTES, curve, raster};
use crate::flatten::{CurveStack, Flatness, Hull, Inspection};
use crate::{FillRule, Path, Point, Segment, Transform, quantize_unit};
use pagura_core::{Error, Meter, Result};

const ROUNDS: i32 = 12;

/// The direct path reserves all but 1/1024 of a coverage level for the
/// ordinary kernel's rounding margin. This is an arithmetic admission
/// bound, not a source-curve approximation allowance.
const DIRECT: f64 = 1.0 / (255.0 * 1024.0);

/// Cached once per source outline and linear map. Kept quadratics remove
/// reduction error, but not cancellation in H or mapping and translation.
#[derive(Debug)]
pub(super) struct Direct {
    /// Largest translation whose arithmetic allowance still fits DIRECT.
    origin: f64,
}

impl Direct {
    pub(super) fn new(outline: &Path, linear: Transform, relative: &Relative) -> Option<Self> {
        if outline
            .segments()
            .iter()
            .any(|s| matches!(s, Segment::CubicTo { .. }))
        {
            return None;
        }
        let pieces = f64::from(
            u32::try_from(
                outline
                    .segments()
                    .len()
                    .max(relative.lines.len() + relative.curves.len()),
            )
            .unwrap_or(u32::MAX),
        );
        let arithmetic = relative
            .curves
            .iter()
            .map(curve::Mono::arithmetic_allowance)
            .fold(0.0, |sum, error| (sum + error).next_up());
        // Solve arithmetic + 384*EPS*pieces*(magnitude + |origin|)
        // <= DIRECT once. Round every inverse step down so the cached
        // translation bound cannot admit more than the original allowance.
        let room = (DIRECT - arithmetic).next_down();
        let magnitude = mapped_magnitude(outline, linear, Point::default());
        let origin = ((room / (384.0 * f64::EPSILON * pieces)).next_down() - magnitude).next_down();
        (origin >= 0.0).then_some(Self { origin })
    }

    fn admits(&self, origin: Point) -> bool {
        origin.x.abs().max(origin.y.abs()) <= self.origin
    }
}

#[derive(Debug, Clone, Copy)]
struct Pixel {
    x: u32,
    y: u32,
    byte: u8,
}

#[derive(Clone, Copy)]
struct Source<'a> {
    outline: &'a Path,
    linear: Transform,
    origin: Point,
    points: &'a [Point],
}

impl<'a> Source<'a> {
    fn of(edges: &'a GlyphEdges, origin: Point, work: &mut Meter) -> Result<Option<Self>> {
        let (outline, linear, points) = match &edges.kind {
            Kind::Curve {
                outline,
                linear,
                point,
                ..
            } => (outline.as_ref(), *linear, point.as_slice()),
            Kind::Polygon {
                source: Some((outline, linear)),
                points,
                ..
            } => (outline.as_ref(), *linear, points.as_slice()),
            Kind::Polygon { source: None, .. } => return Ok(None),
        };
        work.charge(outline.segments().len())?;
        Ok(outline
            .segments()
            .iter()
            .any(|s| matches!(s, Segment::CubicTo { .. } | Segment::QuadTo { .. }))
            .then_some(Self {
                outline,
                linear,
                origin,
                points,
            }))
    }
}

/// A placement leaves this seam in the ordinary shape representation;
/// there are no execution-time corrections or fallible raster operations.
pub(super) fn certify(
    edges: &GlyphEdges,
    origin: Point,
    mut shape: Shape,
    options: &Prep<'_>,
) -> Result<Shape> {
    if exact(edges, origin, &shape, options) {
        return Ok(shape);
    }
    let mut work = Meter::new(options.limits.raster_work, RASTER_WORK);
    let Some(source) = Source::of(edges, origin, &mut work)? else {
        return Ok(shape);
    };
    let Source {
        outline, linear, ..
    } = source;
    let mut bytes = Meter::new(options.limits.render_bytes, RENDER_BYTES);
    bytes.charge(shape.retained_bytes())?;
    let mut targets = Targets::default();
    let (curves, share) = area_share(outline, true);
    let whole = outward(f64::from(u32::try_from(curves).unwrap_or(u32::MAX)) * share);
    let shares = Shares::new(
        outline,
        (linear, share),
        (origin, options.area.region),
        (&mut work, &mut bytes),
    )?;
    let share_bytes = shares.as_ref().map_or(0, Shares::retained_bytes);
    let mut known = Known::default();
    for round in 0..=ROUNDS {
        work.charge_product(outline.segments().len(), 2)?;
        let placement = placement_error(outline, linear, origin, &shape);
        let clip = options
            .clip
            .and_then(|clip| clip.paths())
            .map_or(0.0, Paths::arithmetic_error);
        let bound = match &shares {
            _ if analytic(&shape) => {
                Bound::Whole(curve::reduction_area(outline, linear) + placement + clip)
            }
            Some(shares) => Bound::Shared {
                shares,
                whole,
                rest: placement + clip,
            },
            None => Bound::Whole(whole + placement + clip),
        };
        let pending = ambiguous(&shape, &bound, (&mut work, &mut bytes))?;
        // A pixel's byte from the original outline does not depend on the
        // shape: later rounds look up the pixels earlier ones found.
        known.learn(
            source,
            &pending,
            options,
            Meters {
                work: &mut work,
                bytes: &mut bytes,
            },
        )?;
        let mut wrong = false;
        for pixel in pending {
            if known.byte(pixel) != Some(pixel.byte) {
                wrong = true;
                targets.add(pixel, (&mut work, &mut bytes))?;
            }
        }
        // Inspection cells, pending pixels and row interval geometry have
        // left scope. Keep the shares, the known bytes, the targets and the
        // live shape charged.
        bytes.refund(bytes.used().saturating_sub(
            shape.retained_bytes()
                + targets.retained_bytes()
                + share_bytes
                + known.retained_bytes(),
        ));
        if !wrong {
            return Ok(shape);
        }
        if round == ROUNDS {
            break;
        }
        shape = replacement(
            source,
            shape,
            &targets.boxes,
            (options, round),
            Meters {
                work: &mut work,
                bytes: &mut bytes,
            },
        )?;
    }
    Err(Error::limit("unresolved curve coverage interval"))
}

/// The pixels found wrong so far, as the device boxes a replacement refines
/// its curves at.
#[derive(Default)]
struct Targets {
    boxes: Vec<[f64; 4]>,
}

impl Targets {
    fn add(&mut self, pixel: Pixel, (work, bytes): (&mut Meter, &mut Meter)) -> Result<()> {
        let target = [
            f64::from(pixel.x),
            f64::from(pixel.y),
            f64::from(pixel.x) + 1.0,
            f64::from(pixel.y) + 1.0,
        ];
        work.charge(self.boxes.len())?;
        if !self.boxes.contains(&target) {
            bytes.reserve_capacity(&mut self.boxes, 1)?;
            self.boxes.push(target);
        }
        Ok(())
    }

    fn retained_bytes(&self) -> usize {
        self.boxes.capacity() * size_of::<[f64; 4]>()
    }
}

fn replacement(
    source: Source<'_>,
    shape: Shape,
    targets: &[[f64; 4]],
    (options, round): (&Prep<'_>, i32),
    Meters { work, bytes }: Meters<'_>,
) -> Result<Shape> {
    let before = bytes.used();
    let (polygon, points) = flattened_with(
        source.outline,
        source.linear,
        options.limits,
        bytes,
        Some(Inspection {
            hulls: None,
            work,
            rounding: 0.0,
            targets,
            offset: source.origin,
            factor: 16.0_f64.powi(-round - 1),
        }),
    )?;
    let polygon_bytes = polygon.heap_bytes() + points.capacity() * size_of::<Point>();
    bytes.refund(bytes.used().saturating_sub(before + polygon_bytes));
    // Old geometry stays charged until its replacement is fully ready. As
    // for any polygon placement, only the rows its winding certificate
    // leaves in doubt are resolved; the next round certifies the bytes.
    let prepared = translated(
        (&polygon, &points),
        source.origin,
        Winding::Check(0.0),
        options,
        bytes,
        Some(work),
        None,
    )?
    .ok_or(Error::limit("unresolved curve coverage interval"))?;
    drop((polygon, points));
    bytes.refund(polygon_bytes);
    // The replacement's row resolver and lists have dropped their
    // scratch. Keep only old geometry, targets and the ready replacement
    // charged before the next cell inspection can allocate anything.
    bytes.refund(
        bytes
            .used()
            .saturating_sub(before + prepared.retained_bytes()),
    );
    let released = shape.retained_bytes();
    drop(shape);
    bytes.refund(released);
    Ok(prepared)
}

/// Whether `shape` integrates quadratics in closed form.
fn analytic(shape: &Shape) -> bool {
    match &shape.body {
        Body::Glyph(body) => {
            body.geometry == GlyphGeometry::Relative
                && matches!(body.edges.kind, Kind::Curve { .. })
        }
        Body::Edges(body) => !body.curves.is_empty(),
    }
}

/// Direct integration is reserved for well-conditioned kept quadratics.
/// Ill-conditioned mapping, large pieces or placement cancellation use the
/// measured certificate even though their reduction error is zero.
fn exact(edges: &GlyphEdges, origin: Point, shape: &Shape, options: &Prep<'_>) -> bool {
    analytic(shape)
        && options.clip.and_then(|clip| clip.paths()).is_none()
        && edges
            .direct
            .as_ref()
            .is_some_and(|direct| direct.admits(origin))
}

/// Translation and row/intersection arithmetic can move a boundary. The
/// per-edge homotopy bound is at most three times the two coordinate
/// displacements inside any pixel. This broad initial bound only selects
/// pixels for the local, original-control certificate below.
fn placement_error(outline: &Path, linear: Transform, origin: Point, shape: &Shape) -> f64 {
    let magnitude = mapped_magnitude(outline, linear, origin);
    let pieces = f64::from(
        u32::try_from(outline.segments().len().max(shape.boundaries())).unwrap_or(u32::MAX),
    );
    384.0 * f64::EPSILON * magnitude * pieces
}

fn mapped_magnitude(outline: &Path, linear: Transform, origin: Point) -> f64 {
    // Per axis, so the two maxima do not wait on each other.
    let mut magnitude = [origin.x.abs().max(origin.y.abs()).max(1.0); 2];
    for segment in outline.segments() {
        let mut include = |p: Point| {
            magnitude = [
                magnitude[0].max((p.x * linear.a).abs() + (p.y * linear.c).abs() + origin.x.abs()),
                magnitude[1].max((p.x * linear.b).abs() + (p.y * linear.d).abs() + origin.y.abs()),
            ];
        };
        match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => include(p),
            Segment::CubicTo { c1, c2, end } => {
                include(c1);
                include(c2);
                include(end);
            }
            Segment::QuadTo { control, end } => {
                include(control);
                include(end);
            }
            Segment::Close => {}
        }
    }
    magnitude[0].max(magnitude[1])
}

/// Each curve's share of a polygon's area allowance and the boxes of the
/// hulls that spend it, relative to the glyph origin, with the boxes that
/// can meet each device row of a placement at `origin`.
struct Shares {
    share: f64,
    boxes: Vec<[f64; 4]>,
    origin: Point,
    /// Rows from `first`: the end of each one's boxes in `listed`.
    first: u32,
    ends: Vec<u32>,
    listed: Vec<u32>,
}

impl Shares {
    /// `None` for an outline with cubics, whose reduced quadratics may
    /// leave their control boxes.
    fn new(
        outline: &Path,
        (linear, share): (Transform, f64),
        (origin, region): (Point, DeviceRect),
        (work, bytes): (&mut Meter, &mut Meter),
    ) -> Result<Option<Self>> {
        if outline
            .segments()
            .iter()
            .any(|s| matches!(s, Segment::CubicTo { .. }))
        {
            return Ok(None);
        }
        let mut boxes = Vec::new();
        bytes.reserve(&mut boxes, outline.segments().len())?;
        boxes.extend(crate::flatten::curve_boxes(outline, linear));
        // A box meets a row when it meets the row's band as `within` widens
        // it; the rows one further on each side hold every such row.
        let candidates = |[_, top, _, bottom]: [f64; 4]| {
            rows(top + origin.y - 1.0, bottom + origin.y + 1.0, region)
        };
        let reach = |b: [f64; 4]| {
            candidates(b).into_iter().flat_map(move |(a, z)| {
                (a..=z).filter(move |&y| {
                    b[1] < (f64::from(y) + 1.0 - origin.y).next_up()
                        && b[3] > (f64::from(y) - origin.y).next_down()
                })
            })
        };
        let (first, last) = boxes
            .iter()
            .filter_map(|&b| candidates(b))
            .fold((u32::MAX, 0), |(f, l), (a, z)| (f.min(a), l.max(z)));
        let mut shares = Self {
            share,
            boxes: Vec::new(),
            origin,
            first,
            ends: Vec::new(),
            listed: Vec::new(),
        };
        if first > last {
            shares.boxes = boxes;
            return Ok(Some(shares));
        }
        let count = (last - first + 1) as usize;
        bytes.reserve(&mut shares.ends, count)?;
        shares.ends.resize(count, 0);
        for &b in &boxes {
            for y in reach(b) {
                work.charge(1)?;
                shares.ends[(y - first) as usize] += 1;
            }
        }
        let mut end = 0;
        for slot in &mut shares.ends {
            end += *slot;
            *slot = end;
        }
        bytes.reserve(&mut shares.listed, end as usize)?;
        shares.listed.resize(end as usize, 0);
        // Fill each row's slots from its end, boxes in reverse, so that
        // every row lists its boxes in order and `ends` stays the ends.
        let mut fill = shares.ends.clone();
        for (i, &b) in boxes.iter().enumerate().rev() {
            for y in reach(b) {
                let slot = &mut fill[(y - first) as usize];
                *slot -= 1;
                shares.listed[*slot as usize] =
                    u32::try_from(i).map_err(|_| Error::limit("glyph curves exceed u32"))?;
            }
        }
        shares.boxes = boxes;
        Ok(Some(shares))
    }

    /// The area allowance the hulls of the placement can spend in pixels
    /// `x0..x1` of row `y`.
    fn within(&self, y: u32, (x0, x1): (u32, u32)) -> f64 {
        let origin = self.origin;
        let pixels = [
            (f64::from(x0) - origin.x).next_down(),
            (f64::from(y) - origin.y).next_down(),
            (f64::from(x1) - origin.x).next_up(),
            (f64::from(y) + 1.0 - origin.y).next_up(),
        ];
        let meeting = self.row(y).iter().filter(|&&i| {
            Hull {
                bounds: self.boxes[i as usize],
                area: 0.0,
            }
            .intersects(pixels)
        });
        outward(f64::from(u32::try_from(meeting.count()).unwrap_or(u32::MAX)) * self.share)
    }

    /// The boxes that can meet row `y`.
    fn row(&self, y: u32) -> &[u32] {
        let row = y
            .checked_sub(self.first)
            .and_then(|r| usize::try_from(r).ok())
            .filter(|&r| r < self.ends.len());
        row.map_or(&[][..], |r| {
            let start = r.checked_sub(1).map_or(0, |r| self.ends[r] as usize);
            self.listed.get(start..self.ends[r] as usize).unwrap_or(&[])
        })
    }

    fn retained_bytes(&self) -> usize {
        self.boxes.capacity() * size_of::<[f64; 4]>()
            + (self.ends.capacity() + self.listed.capacity()) * size_of::<u32>()
    }
}

/// How far a placement's area can be from the original outline's, apart
/// from the arithmetic of its cells.
enum Bound<'a> {
    /// One bound for every pixel.
    Whole(f64),
    /// The polygon's area allowance over the whole outline, which a pixel's
    /// curves can spend only as far as their hulls meet it, plus `rest`.
    Shared {
        shares: &'a Shares,
        whole: f64,
        rest: f64,
    },
}

/// The pixels whose byte the shape's area within `bound` leaves in doubt.
fn ambiguous(
    shape: &Shape,
    bound: &Bound<'_>,
    (work, bytes): (&mut Meter, &mut Meter),
) -> Result<Vec<Pixel>> {
    let width = match &shape.body {
        Body::Glyph(body) => body.scan.1.saturating_sub(body.scan.0),
        Body::Edges(body) => body.scan.1.saturating_sub(body.scan.0),
    };
    let mut cells = Cells::new(width, bytes)?;
    let cell_bytes = cells.retained_bytes();
    work.charge(shape.work())?;
    let mut pending = Vec::new();
    let mut failure = None;
    raster::inspect(shape, &mut cells, |y, x0, x1, area, arithmetic| {
        // Signed winding can exceed one in the canonical point cell.
        // Clamp the certificate center before adding uncertainty, so a
        // saturated raw area cannot hide an original-control error.
        let center = area.clamp(0.0, 1.0);
        let certain = |error: f64| {
            let error = outward(error + arithmetic);
            area.is_finite()
                && error.is_finite()
                && quantize_unit(center - error) == quantize_unit(center + error)
        };
        let whole = match *bound {
            Bound::Whole(error) => error,
            Bound::Shared { whole, rest, .. } => whole + rest,
        };
        if failure.is_some() || certain(whole) {
            return;
        }
        let result = (|| {
            if let Bound::Shared { shares, rest, .. } = *bound {
                work.charge(shares.row(y).len())?;
                let column =
                    |x| u32::try_from(x).map_err(|_| Error::limit("glyph column exceeds u32"));
                let columns = (column(x0)?, column(x1)?);
                if certain(shares.within(y, columns) + rest) {
                    return Ok(());
                }
            }
            work.charge(x1 - x0)?;
            bytes.reserve_capacity(&mut pending, x1 - x0)?;
            for x in x0..x1 {
                pending.push(Pixel {
                    x: u32::try_from(x).map_err(|_| Error::limit("glyph column exceeds u32"))?,
                    y,
                    byte: quantize_unit(area),
                });
            }
            Ok(())
        })();
        if let Err(error) = result {
            failure = Some(error);
        }
    })?;
    drop(cells);
    bytes.refund(cell_bytes);
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(pending)
}

/// The bytes of the pixels met so far, from the original outline, by row
/// and column, and the outline's items by height once a row needs them.
#[derive(Default)]
struct Known {
    pixels: Vec<(u32, u32, u8)>,
    rows: Option<Rows>,
}

impl Known {
    /// Finds the bytes of the pixels of `pending` not known yet, the
    /// pixels of a row together.
    fn learn(
        &mut self,
        source: Source<'_>,
        pending: &[Pixel],
        options: &Prep<'_>,
        Meters { work, bytes }: Meters<'_>,
    ) -> Result<()> {
        let mut new = Vec::new();
        bytes.reserve(&mut new, pending.len())?;
        new.extend(
            pending
                .iter()
                .filter(|&&pixel| self.byte(pixel).is_none())
                .map(|pixel| (pixel.y, pixel.x)),
        );
        if new.is_empty() {
            return Ok(());
        }
        work.charge(new.len().saturating_mul(new.len().ilog2() as usize + 2))?;
        new.sort_unstable();
        let Self { pixels, rows } = self;
        let rows = made(rows, source, bytes)?;
        bytes.reserve_capacity(pixels, new.len())?;
        let (mut columns, mut found) = (Vec::new(), Vec::new());
        for row in new.chunk_by(|a, b| a.0 == b.0) {
            columns.clear();
            bytes.reserve_capacity(&mut columns, row.len())?;
            columns.extend(row.iter().map(|&(_, x)| x));
            row_bytes(
                source,
                rows,
                (row[0].0, &columns),
                options,
                (work, bytes),
                &mut found,
            )?;
            pixels.extend(row.iter().zip(&found).map(|(&(y, x), &byte)| (y, x, byte)));
        }
        pixels.sort_unstable();
        Ok(())
    }

    fn byte(&self, pixel: Pixel) -> Option<u8> {
        self.pixels
            .binary_search_by(|&(y, x, _)| (y, x).cmp(&(pixel.y, pixel.x)))
            .ok()
            .map(|i| self.pixels[i].2)
    }

    fn retained_bytes(&self) -> usize {
        self.pixels.capacity() * size_of::<(u32, u32, u8)>()
            + self.rows.as_ref().map_or(0, Rows::retained_bytes)
    }
}

/// The outline's items by box, made once.
fn made<'a>(rows: &'a mut Option<Rows>, source: Source<'_>, bytes: &mut Meter) -> Result<&'a Rows> {
    match rows {
        Some(rows) => Ok(rows),
        None => Ok(rows.insert(Rows::new(
            source.outline,
            source.linear,
            slack(source),
            bytes,
        )?)),
    }
}

/// More than the rounding of any frame a placement's rows are built in.
/// Coordinates there stay within twice the placement's magnitude plus one;
/// hull, mapping and interpolation allowances stay below 3000 rounding
/// units of that.
fn slack(source: Source<'_>) -> f64 {
    8192.0 * f64::EPSILON * (mapped_magnitude(source.outline, source.linear, source.origin) + 1.0)
}

/// The bytes of pixels `columns` (ascending) of row `y` into `found`, each
/// refined from the original outline until its interval leaves it certain.
fn row_bytes(
    source: Source<'_>,
    rows: &Rows,
    (y, columns): (u32, &[u32]),
    options: &Prep<'_>,
    (work, bytes): (&mut Meter, &mut Meter),
    found: &mut Vec<u8>,
) -> Result<()> {
    found.clear();
    bytes.reserve_capacity(found, columns.len())?;
    found.resize(columns.len(), 0);
    let (mut open, mut at, mut intervals) = (Vec::new(), Vec::new(), Vec::new());
    bytes.reserve(&mut open, columns.len())?;
    bytes.reserve(&mut at, columns.len())?;
    bytes.reserve(&mut intervals, columns.len())?;
    open.extend(0..columns.len());
    for round in 0..=ROUNDS {
        at.clear();
        at.extend(open.iter().map(|&i| columns[i]));
        intervals.clear();
        intervals.resize(at.len(), (0.0, 0.0));
        let before = bytes.used();
        let result = row_intervals(
            source,
            rows,
            (y, &at),
            (options, round),
            (work, bytes),
            &mut intervals,
        );
        // All interval geometry and resolver scratch has left scope.
        bytes.refund(bytes.used().saturating_sub(before));
        result?;
        let mut still = 0;
        for k in 0..open.len() {
            let (area, error) = intervals[k];
            let lo = quantize_unit(area - error);
            if lo == quantize_unit(area + error) {
                found[open[k]] = lo;
            } else {
                open[still] = open[k];
                still += 1;
            }
        }
        open.truncate(still);
        if open.is_empty() {
            return Ok(());
        }
    }
    Err(Error::limit("unresolved curve coverage interval"))
}

/// Each pixel's area within the original outline's covered set and its
/// error, for pixels `columns` (ascending) of row `y`, into `out`: from one
/// flattening of the outline's items that reach the row, refined where
/// they meet the pixels. The row's frame puts the first pixel at the unit
/// square, its translation split off error-free; pixel `k` columns further
/// lies at `k..k + 1`.
fn row_intervals(
    source: Source<'_>,
    rows: &Rows,
    (y, columns): (u32, &[u32]),
    (options, round): (&Prep<'_>, i32),
    (work, bytes): (&mut Meter, &mut Meter),
    out: &mut [(f64, f64)],
) -> Result<()> {
    let Source {
        outline,
        linear,
        origin,
        points: source_points,
    } = source;
    let Some(&first) = columns.first() else {
        return Ok(());
    };
    let (ox, dx) = subtract(origin.x, f64::from(first));
    let (oy, dy) = subtract(origin.y, f64::from(y));
    let local = Transform {
        e: ox,
        f: oy,
        ..linear
    };
    let frame = Frame::new(options).ok_or(Error::limit("unresolved curve coverage interval"))?;
    let row = RowFrame {
        at: (first, y),
        columns,
    };
    let mut targets = Vec::new();
    bytes.reserve(&mut targets, columns.len())?;
    targets.extend(
        columns
            .iter()
            .map(|&x| clip_bounds(&frame, row.at, row.column(x))),
    );
    // The outline's heights under the linear map that reach this row.
    let band = [-oy, 1.0 - oy];
    let mut hulls = Vec::new();
    let mut lines = Vec::new();
    rows.lines(
        band,
        Making {
            transform: local,
            shift: None,
            flatness: Flatness::Area(area_share(outline, linear.determinant() != 0.0).1),
            inspection: Inspection {
                hulls: Some(&mut hulls),
                work,
                rounding: dx.abs().max(dy.abs()) * (1.0 + 8.0 * f64::EPSILON),
                targets: &targets,
                offset: Point::default(),
                factor: 16.0_f64.powi(-round),
            },
        },
        options.limits,
        (&mut Vec::new(), &mut CurveStack::new()),
        (&mut lines, bytes),
    )?;
    for &point in source_points {
        // A genuine point's floor belongs to its canonical placed position.
        let cell = Point::new(
            (point.x + origin.x).floor() - f64::from(first),
            (point.y + origin.y).floor() - f64::from(y),
        );
        bytes.reserve_capacity(&mut lines, 2)?;
        lines.extend(point_sides(cell));
    }
    let mut errors = row.errors(
        (source, local),
        (rows, band),
        (&hulls, &lines, &targets),
        (work, bytes),
    )?;
    let mut pieces = Vec::new();
    row.gathered(&lines, (&mut pieces, bytes), work)?;
    drop((hulls, lines));
    let mut rules = Vec::new();
    bytes.reserve_capacity(&mut rules, 1)?;
    rules.push(FillRule::NonZero);
    clip_pieces(
        &frame,
        &row,
        (&mut pieces, &mut rules),
        Meters { work, bytes },
        &mut errors,
    )?;
    let rounding = row.areas((&pieces, &rules), (work, bytes), out)?;
    for (slot, [hull, arithmetic, source, clip]) in out.iter_mut().zip(errors) {
        let error = outward(hull + arithmetic + source);
        slot.1 = outward(outward(error + clip) + rounding);
    }
    Ok(())
}

/// A row's frame: its first pixel's device column and row, at the unit
/// square, and the pending pixels' columns.
struct RowFrame<'a> {
    at: (u32, u32),
    columns: &'a [u32],
}

impl RowFrame<'_> {
    /// Where device column `x` starts in the frame: an integer, exact.
    fn column(&self, x: u32) -> f64 {
        f64::from(x - self.at.0)
    }

    fn unit(&self, i: usize) -> [f64; 4] {
        let x = self.column(self.columns[i]);
        [x, 0.0, x + 1.0, 1.0]
    }

    /// The pieces of `lines` in the row that a pixel's cells can depend on.
    ///
    /// A pixel's cell takes the pieces within its column and, from those
    /// left of it, only the windings they leave at its left side. A piece
    /// between two pixels' columns, or left of the first, therefore stands
    /// for nothing but its direction over its heights: such pieces become
    /// vertical pieces inside their gap that carry the same windings, a
    /// unit of winding each, and pieces right of the last pixel go. Every
    /// other piece stays as it is. Each piece's columns are widened by its
    /// interpolation error, so a piece taken out cannot reach a pixel.
    fn gathered(
        &self,
        lines: &[Line],
        (pieces, bytes): (&mut Vec<Piece>, &mut Meter),
        work: &mut Meter,
    ) -> Result<()> {
        let mut steps = Vec::new();
        work.charge(lines.len())?;
        for &line in lines {
            let Some(piece) = Piece::new(line, 0.0, 0) else {
                continue;
            };
            let (a, b) = (line.x(piece.lo), line.x(piece.hi));
            let error = line.interpolation_error().x;
            let (lo, hi) = (a.min(b) - error, a.max(b) + error);
            let gap = self.columns.partition_point(|&x| self.column(x) + 1.0 < lo);
            match self.columns.get(gap) {
                None => {}
                Some(&x) if hi < self.column(x) => {
                    bytes.reserve_capacity(&mut steps, 2)?;
                    steps.push((gap, piece.lo, piece.direction));
                    steps.push((gap, piece.hi, -piece.direction));
                }
                Some(_) => {
                    bytes.reserve_capacity(pieces, 1)?;
                    pieces.push(piece);
                }
            }
        }
        work.charge(
            steps
                .len()
                .saturating_mul(steps.len().max(1).ilog2() as usize + 2),
        )?;
        steps.sort_unstable_by(|p, q| p.0.cmp(&q.0).then(p.1.total_cmp(&q.1)));
        for gap in steps.chunk_by(|p, q| p.0 == q.0) {
            let first = gap[0].0;
            // Winding units: the most the gap carries at any height.
            let mut winding = 0_i32;
            let mut units = 0;
            for height in gap.chunk_by(|p, q| p.1 == q.1) {
                winding += height.iter().map(|step| step.2).sum::<i32>();
                units = units.max(winding.unsigned_abs());
            }
            let right = self.column(self.columns[first]);
            let left = match first.checked_sub(1) {
                Some(previous) => self.column(self.columns[previous]) + 1.0,
                None => right - f64::from(units) - 1.0,
            };
            let place = |unit: u32| left + (right - left) * f64::from(unit) / f64::from(units + 1);
            // Per unit, the height it opened at and its sign.
            let mut open = Vec::new();
            bytes.reserve(&mut open, units as usize)?;
            open.resize(units as usize, None::<(f64, i32)>);
            let mut close = |unit: usize, from: f64, to: f64, sign: i32| -> Result<()> {
                let x = place(u32::try_from(unit + 1).unwrap_or(u32::MAX));
                let (a, b) = (Point::new(x, from), Point::new(x, to));
                let line = if sign > 0 {
                    Line { a, b }
                } else {
                    Line { a: b, b: a }
                };
                if let Some(piece) = Piece::new(line, 0.0, 0) {
                    bytes.reserve_capacity(pieces, 1)?;
                    pieces.push(piece);
                }
                Ok(())
            };
            winding = 0;
            for height in gap.chunk_by(|p, q| p.1 == q.1) {
                let y = height[0].1;
                winding += height.iter().map(|step| step.2).sum::<i32>();
                let (sign, level) = (winding.signum(), winding.unsigned_abs() as usize);
                for (unit, slot) in open.iter_mut().enumerate() {
                    match *slot {
                        Some((_, kept)) if unit < level && kept == sign => {}
                        Some((from, kept)) => {
                            close(unit, from, y, kept)?;
                            *slot = (unit < level).then_some((y, sign));
                        }
                        None if unit < level => *slot = Some((y, sign)),
                        None => {}
                    }
                }
            }
        }
        Ok(())
    }

    /// Per pixel, the errors of its area that the hulls, the lines'
    /// arithmetic and the straight source sides bound, each where it can
    /// reach the pixel, and a place for the clip's.
    fn errors(
        &self,
        (source, local): (Source<'_>, Transform),
        (rows, band): (&Rows, [f64; 2]),
        (hulls, lines, targets): (&[Hull], &[Line], &[[f64; 4]]),
        (work, bytes): (&mut Meter, &mut Meter),
    ) -> Result<Vec<[f64; 4]>> {
        let mut errors = Vec::new();
        bytes.reserve(&mut errors, self.columns.len())?;
        errors.resize(self.columns.len(), [0.0_f64; 4]);
        work.charge(hulls.len().saturating_add(lines.len()))?;
        for &hull in hulls {
            self.spread(hull, |i| targets[i], (&mut errors, 0));
        }
        for &line in lines {
            self.spread(
                edge_hull(line, line.interpolation_error()),
                |i| self.unit(i),
                (&mut errors, 1),
            );
        }
        rows.sides(band, work, |a, b| {
            self.spread(
                source_side(source, local, self.at, (a, b)),
                |i| targets[i],
                (&mut errors, 2),
            );
        })?;
        Ok(errors)
    }

    /// Each pixel's area within the covered set of `pieces` into `out`,
    /// and the arithmetic error of the cells that measure them.
    fn areas(
        &self,
        (pieces, rules): (&[Piece], &[FillRule]),
        (work, bytes): (&mut Meter, &mut Meter),
        out: &mut [(f64, f64)],
    ) -> Result<f64> {
        work.charge(pieces.len())?;
        let mut sides = Vec::new();
        Resolver::default().row(pieces, rules, 0.0, (&mut sides, Meters { work, bytes }))?;
        let width = self
            .columns
            .last()
            .map_or(0, |&x| (x - self.at.0) as usize + 1);
        let mut cells = Cells::new(width, bytes)?;
        if !cells.open((0, width), 0..1) {
            return Err(Error::limit("curve interval cells exceed allocation"));
        }
        work.charge_product(sides.len(), 8)?;
        for side in sides {
            cells.line_measured(side);
        }
        let mut i = 0;
        cells.inspect(0, |_, x1, value| {
            while let Some(&x) = self.columns.get(i)
                && ((x - self.at.0) as usize) < x1
            {
                out[i].0 = value.abs();
                i += 1;
            }
        });
        Ok(cells.arithmetic_error())
    }

    /// Adds `reach.area` to error `term` of every pixel whose `bounds`
    /// `reach` meets, as each pixel's own interval would.
    fn spread(
        &self,
        reach: Hull,
        bounds: impl Fn(usize) -> [f64; 4],
        (errors, term): (&mut [[f64; 4]], usize),
    ) {
        let [left, _, right, _] = reach.bounds;
        let from = self
            .columns
            .partition_point(|&x| self.column(x) + 1.0 <= left);
        let to = self.columns.partition_point(|&x| self.column(x) < right);
        for (i, error) in errors.iter_mut().enumerate().take(to).skip(from) {
            if reach.intersects(bounds(i)) {
                error[term] = outward(error[term] + reach.area);
            }
        }
    }
}

/// The clip's pieces in the row's frame, as further groups with their rules,
/// and their localization and interpolation errors per pixel (term 3).
fn clip_pieces(
    frame: &Frame<'_>,
    row: &RowFrame<'_>,
    (pieces, rules): (&mut Vec<Piece>, &mut Vec<FillRule>),
    Meters { work, bytes }: Meters<'_>,
    errors: &mut [[f64; 4]],
) -> Result<()> {
    let add = |line: Line, bound: Point, errors: &mut [[f64; 4]]| {
        for delta in [bound, line.interpolation_error()] {
            row.spread(edge_hull(line, delta), |i| row.unit(i), (&mut *errors, 3));
        }
    };
    if let Some(rect) = frame.rect {
        let group =
            u8::try_from(rules.len()).map_err(|_| Error::limit("clip path count exceeds u8"))?;
        bytes.reserve_capacity(rules, 1)?;
        rules.push(FillRule::NonZero);
        let corners = [
            Point::new(rect.left, rect.top),
            Point::new(rect.right, rect.top),
            Point::new(rect.right, rect.bottom),
            Point::new(rect.left, rect.bottom),
        ];
        for i in 0..4 {
            let (line, bound) = localized_line(
                Line {
                    a: corners[i],
                    b: corners[(i + 1) % 4],
                },
                row.at,
            );
            add(line, bound, errors);
            if let Some(piece) = Piece::new(line, 0.0, group) {
                bytes.reserve_capacity(pieces, 1)?;
                pieces.push(piece);
            }
        }
    }
    if let Some(paths) = frame.paths {
        let group =
            u8::try_from(rules.len()).map_err(|_| Error::limit("clip path count exceeds u8"))?;
        let start = pieces.len();
        paths.pieces(row.at.1, group, (pieces, bytes), work)?;
        for piece in pieces.get_mut(start..).unwrap_or_default() {
            let (line, bound) = localized_line(piece.line, row.at);
            add(line, bound, errors);
            *piece = Piece::new(line, 0.0, piece.group)
                .ok_or(Error::limit("unresolved clip localization interval"))?;
        }
        let count = paths.rules().count();
        bytes.reserve_capacity(rules, count)?;
        rules.extend(paths.rules());
    }
    Ok(())
}

/// Error-free two-sum subtraction, retaining the exact stored-coordinate
/// difference even when the integer pixel lies far from an endpoint.
fn subtract(a: f64, b: f64) -> (f64, f64) {
    let high = a - b;
    let back = high - a;
    (high, (a - (high - back)) + (-b - back))
}

fn outward(value: f64) -> f64 {
    value * (1.0 + 16.0 * f64::EPSILON)
}

/// `line` relative to device pixel `(x, y)`, and the largest tails its
/// endpoints' subtractions dropped.
fn localized_line(line: Line, (x, y): (u32, u32)) -> (Line, Point) {
    let shift = |p: Point| {
        let (px, dx) = subtract(p.x, f64::from(x));
        let (py, dy) = subtract(p.y, f64::from(y));
        (Point::new(px, py), Point::new(dx.abs(), dy.abs()))
    };
    let (a, da) = shift(line.a);
    let (b, db) = shift(line.b);
    (Line { a, b }, Point::new(da.x.max(db.x), da.y.max(db.y)))
}

/// The area `line` can sweep when each endpoint moves by up to `delta`, and
/// the box it sweeps within: the error it adds to the pixels the box meets.
fn edge_hull(line: Line, delta: Point) -> Hull {
    let radius = delta.x.max(delta.y);
    let bounds = [
        (line.a.x.min(line.b.x) - radius).next_down(),
        (line.a.y.min(line.b.y) - radius).next_down(),
        (line.a.x.max(line.b.x) + radius).next_up(),
        (line.a.y.max(line.b.y) + radius).next_up(),
    ];
    // Each coordinate of the linear homotopy is monotone. Perturbation
    // adds at most two radii to its variation in a unit pixel; tangential
    // displacement of an axis-aligned edge sweeps no area.
    let area = outward(
        4.0 * (delta.x * ((line.b.y - line.a.y).abs() + 2.0 * delta.y).min(1.0)
            + delta.y * ((line.b.x - line.a.x).abs() + 2.0 * delta.x).min(1.0))
            + 16.0 * delta.x * delta.y,
    );
    Hull { bounds, area }
}

/// The clipped part of the pixel at `column` of a row's frame whose first
/// pixel is device pixel `at`.
fn clip_bounds(frame: &Frame<'_>, (x, y): (u32, u32), column: f64) -> [f64; 4] {
    let mut bounds = [column, 0.0_f64, column + 1.0, 1.0];
    let clips = frame
        .rect
        .into_iter()
        .chain(frame.paths.and_then(Paths::proof_bounds));
    for Bounds {
        left,
        top,
        right,
        bottom,
    } in clips
    {
        let local = |a, b, lower| {
            let (high, tail): (f64, f64) = subtract(a, b);
            if lower {
                (high - tail.abs()).next_down()
            } else {
                (high + tail.abs()).next_up()
            }
        };
        bounds[0] = bounds[0].max(local(left, f64::from(x), true));
        bounds[1] = bounds[1].max(local(top, f64::from(y), true));
        bounds[2] = bounds[2].min(local(right, f64::from(x), false));
        bounds[3] = bounds[3].min(local(bottom, f64::from(y), false));
    }
    bounds
}

/// The area between straight source side `a`–`b`, at its canonical place,
/// and its line in the row's frame under `local`.
fn source_side(
    source: Source<'_>,
    local: Transform,
    at: (u32, u32),
    (a, b): (Point, Point),
) -> Hull {
    let canonical = |p: Point| {
        let p = p * source.linear;
        Point::new(p.x + source.origin.x, p.y + source.origin.y)
    };
    let (target, tail) = localized_line(
        Line {
            a: canonical(a),
            b: canonical(b),
        },
        at,
    );
    let placed = Line {
        a: a * local,
        b: b * local,
    };
    let delta = Point::new(
        outward(
            (target.a.x - placed.a.x)
                .abs()
                .max((target.b.x - placed.b.x).abs())
                + tail.x,
        ),
        outward(
            (target.a.y - placed.a.y)
                .abs()
                .max((target.b.y - placed.b.y).abs())
                + tail.y,
        ),
    );
    edge_hull(placed, delta)
}

#[cfg(test)]
mod tests {
    use super::super::AREA;
    use super::*;
    use crate::{Area, ClipShape, DeviceRect, PathBuilder, ShapeOptions};
    use pagura_core::Limits;

    /// One pixel's interval, as a row of one pixel.
    fn interval(
        source: Source<'_>,
        pixel: Pixel,
        (options, round): (&Prep<'_>, i32),
        (work, bytes): (&mut Meter, &mut Meter),
    ) -> Result<(f64, f64)> {
        let rows = Rows::new(source.outline, source.linear, slack(source), bytes)?;
        let mut out = [(0.0, 0.0)];
        row_intervals(
            source,
            &rows,
            (pixel.y, &[pixel.x]),
            (options, round),
            (work, bytes),
            &mut out,
        )?;
        Ok(out[0])
    }

    fn true_byte(
        source: Source<'_>,
        pixel: Pixel,
        options: &Prep<'_>,
        (work, bytes): (&mut Meter, &mut Meter),
    ) -> Result<u8> {
        let rows = Rows::new(source.outline, source.linear, slack(source), bytes)?;
        let mut found = Vec::new();
        row_bytes(
            source,
            &rows,
            (pixel.y, &[pixel.x]),
            options,
            (work, bytes),
            &mut found,
        )?;
        Ok(found[0])
    }

    #[test]
    fn a_pixel_spends_only_the_shares_of_the_curves_that_reach_it() {
        // Two quadratic bumps far apart, then a straight side.
        let mut path = PathBuilder::new();
        path.move_to(Point::new(0.0, 0.0));
        path.quad_to(Point::new(1.0, 2.0), Point::new(2.0, 0.0))
            .unwrap();
        path.line_to(Point::new(20.0, 0.0)).unwrap();
        path.quad_to(Point::new(21.0, 2.0), Point::new(22.0, 0.0))
            .unwrap();
        path.close();
        let path = path.finish();
        let share = area_share(&path, true).1;
        let origin = Point::new(100.25, 50.5);
        let region = DeviceRect {
            x0: 0,
            y0: 0,
            x1: 200,
            y1: 100,
        };
        let shares = Shares::new(
            &path,
            (Transform::IDENTITY, share),
            (origin, region),
            (
                &mut Meter::new(usize::MAX, RASTER_WORK),
                &mut crate::testing::bytes(),
            ),
        )
        .unwrap()
        .unwrap();
        assert_eq!(shares.boxes.len(), 2);
        assert_eq!(share, AREA / 2.0);
        let within = |x: u32, y: u32, n: u32| shares.within(y, (x, x + n));
        assert_eq!(within(110, 50, 1), 0.0);
        assert!((share..1.01 * share).contains(&within(101, 50, 1)));
        assert!((2.0 * share..1.01 * AREA).contains(&within(100, 50, 30)));
        // Below the bumps' boxes.
        assert_eq!(within(100, 60, 30), 0.0);
        // A certified polygon shares the allowance among at most eight
        // curves; one used as it is, among all of them.
        let mut many = PathBuilder::new();
        many.move_to(Point::new(0.0, 0.0));
        for i in 0..20 {
            let x = f64::from(i);
            many.quad_to(Point::new(x + 0.5, 1.0), Point::new(x + 1.0, 0.0))
                .unwrap();
        }
        let many = many.finish();
        assert_eq!(area_share(&many, true), (20, AREA / 8.0));
        assert_eq!(area_share(&many, false), (20, AREA / 20.0));
        // Reduced cubics may leave their control boxes.
        let mut cubic = PathBuilder::new();
        cubic.move_to(Point::new(0.0, 0.0));
        cubic
            .cubic_to(
                Point::new(1.0, 1.0),
                Point::new(2.0, 1.0),
                Point::new(3.0, 0.0),
            )
            .unwrap();
        let cubic = cubic.finish();
        let none = Shares::new(
            &cubic,
            (Transform::IDENTITY, share),
            (origin, region),
            (
                &mut Meter::new(usize::MAX, RASTER_WORK),
                &mut crate::testing::bytes(),
            ),
        );
        assert!(none.unwrap().is_none());
    }

    #[test]
    fn mixed_rows_keep_original_curve_and_point_enclosures() {
        let limits = Limits::default();
        let origin = Point::new(0.0, 11.0 / 18.0 + 1e-10);
        for reversed in [false, true] {
            let mut path = PathBuilder::new();
            if reversed {
                path.move_to(Point::new(0.0, 7.0));
                path.line_to(Point::new(6.0, 7.0)).unwrap();
                path.line_to(Point::new(6.0, 0.0)).unwrap();
                path.cubic_to(Point::new(4.0, 4.0), Point::new(2.0, 4.0), Point::default())
                    .unwrap();
            } else {
                path.move_to(Point::default());
                path.cubic_to(
                    Point::new(2.0, 4.0),
                    Point::new(4.0, 4.0),
                    Point::new(6.0, 0.0),
                )
                .unwrap();
                path.line_to(Point::new(6.0, 7.0)).unwrap();
                path.line_to(Point::new(0.0, 7.0)).unwrap();
            }
            path.close();
            path.move_to(Point::new(2.25, 4.25));
            path.close();
            let path = path.finish();
            let edges =
                std::sync::Arc::new(GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
            for band_height in [1, 3, 16, 32] {
                let options = Prep {
                    area: Area {
                        region: DeviceRect {
                            x0: 0,
                            y0: 0,
                            x1: 8,
                            y1: 10,
                        },
                        band_height,
                    },
                    limits: &limits,
                    clip: None,
                };
                let shape = super::super::placed(&edges, origin, &options)
                    .unwrap()
                    .unwrap();
                assert!(shape.resolved.is_empty());
                let relative = edges.relative().unwrap();
                assert_eq!(
                    shape.boundaries(),
                    relative.lines.len() + relative.curves.len() + 2
                );
                let ordinary = placement_error(&path, Transform::IDENTITY, origin, &shape);
                let expected = 384.0
                    * f64::EPSILON
                    * (7.0 + origin.y)
                    * f64::from(
                        u32::try_from(path.segments().len().max(shape.boundaries())).unwrap(),
                    );
                assert_eq!(ordinary.to_bits(), expected.to_bits());
                // The original cubic has x=6t and y=12t(1-t). Over x=2..3,
                // its mean height is26/9, so the covered part of row3 is
                // 4-origin.y-26/9. This evaluation avoids the raster kernel.
                let original = 10.0 / 9.0 - origin.y;
                assert_eq!(quantize_unit(original), 127);
                let mut found = false;
                let mut cells = Cells::new(8, &mut crate::testing::bytes()).unwrap();
                raster::inspect(&shape, &mut cells, |y, x0, x1, value, arithmetic| {
                    if y == 3 && x0 <= 2 && x1 > 2 {
                        found = true;
                        let error = ordinary + arithmetic + 8.0 * f64::EPSILON;
                        assert!((value.abs() - original).abs() <= error);
                    }
                })
                .unwrap();
                assert!(found);
                let ready = certify(&edges, origin, shape, &options).unwrap();
                let (coverage, _) = crate::testing::raster(&options.area, Some(&ready), None);
                assert_eq!(coverage[3 * 8 + 2], 127);
                assert_eq!(coverage[4 * 8 + 2], if reversed { 0 } else { 255 });
            }
        }
    }

    #[test]
    fn saturated_winding_keeps_original_control_and_nonfinite_uncertainty() {
        let limits = Limits::default();
        let mut path = PathBuilder::new();
        path.move_to(Point::default());
        path.cubic_to(
            Point::new(2.0, 4.0),
            Point::new(4.0, 4.0),
            Point::new(6.0, 0.0),
        )
        .unwrap();
        path.line_to(Point::new(6.0, 7.0)).unwrap();
        path.line_to(Point::new(0.0, 7.0)).unwrap();
        path.close();
        path.move_to(Point::new(2.25, 4.25));
        path.close();
        let edges = std::sync::Arc::new(
            GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap(),
        );
        let options = Prep {
            area: Area {
                region: DeviceRect {
                    x0: 0,
                    y0: 0,
                    x1: 8,
                    y1: 8,
                },
                band_height: 3,
            },
            limits: &limits,
            clip: None,
        };
        let shape = super::super::placed(&edges, Point::default(), &options)
            .unwrap()
            .unwrap();
        let mut cells = Cells::new(8, &mut crate::testing::bytes()).unwrap();
        let mut found = false;
        raster::inspect(&shape, &mut cells, |y, x0, x1, area, _| {
            if y == 4 && x0 <= 2 && x1 > 2 {
                found = true;
                assert!(area > 1.5);
            }
        })
        .unwrap();
        assert!(found);
        for uncertainty in [1.0 / 255.0, f64::NAN, f64::INFINITY] {
            let mut work = Meter::new(limits.raster_work, RASTER_WORK);
            let mut bytes = crate::testing::bytes();
            let pending =
                ambiguous(&shape, &Bound::Whole(uncertainty), (&mut work, &mut bytes)).unwrap();
            let pixel = *pending.iter().find(|p| p.x == 2 && p.y == 4).unwrap();
            assert_eq!(pixel.byte, 255);
            let source = Source::of(&edges, Point::default(), &mut work)
                .unwrap()
                .unwrap();
            assert_eq!(
                true_byte(source, pixel, &options, (&mut work, &mut bytes)).unwrap(),
                255
            );
        }
    }

    #[test]
    fn local_proof_chords_keep_nonpoint_loops_distinct_from_points() {
        let limits = Limits::default();
        let area = Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 1,
            },
            band_height: 1,
        };
        let mut clip_path = PathBuilder::new();
        clip_path.rect(0.75, 0.75, 0.25, 0.25);
        let clip_path = clip_path.finish();
        let clip = ClipShape::new(
            &[(&clip_path, FillRule::NonZero)],
            Transform::IDENTITY,
            &ShapeOptions {
                area,
                limits: &limits,
            },
        )
        .unwrap();
        let options = Prep {
            area,
            limits: &limits,
            clip: Some(&clip),
        };
        let start = Point::new(0.125, 0.125);
        for (control, expected) in [(Point::new(0.25, 0.25), 0), (start, 16)] {
            let mut path = PathBuilder::new();
            path.move_to(start);
            path.cubic_to(control, control, start).unwrap();
            path.close();
            let path = path.finish();
            let edges = GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap();
            let mut work = Meter::new(limits.raster_work, RASTER_WORK);
            let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
            let source = Source::of(&edges, Point::default(), &mut work)
                .unwrap()
                .unwrap();
            let (value, error) = interval(
                source,
                Pixel {
                    x: 0,
                    y: 0,
                    byte: 0,
                },
                (&options, 0),
                (&mut work, &mut bytes),
            )
            .unwrap();
            // The out-and-back curve is nonpoint geometry outside the
            // partial clip. A genuine point still owns its whole floor cell.
            assert_eq!(quantize_unit(value - error), expected);
            assert_eq!(quantize_unit(value + error), expected);
        }
    }

    #[test]
    fn analytic_integral_cancellation_cannot_skip_a_coverage_byte() {
        let limits = Limits::default();
        let m = 98_304.0;
        let mut path = PathBuilder::new();
        path.move_to(Point::default());
        path.cubic_to(
            Point::new(m / 6.0, m / 3.0),
            Point::new(m / 2.0, 2.0 * m / 3.0),
            Point::new(m, m),
        )
        .unwrap();
        path.line_to(Point::new(m + 2.0, m)).unwrap();
        path.line_to(Point::new(m + 2.0, 0.0)).unwrap();
        path.close();
        let path = path.finish();
        let edges =
            std::sync::Arc::new(GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
        assert!(edges.curved());
        let origin = Point::new(-98_302.997_063_444_05, 1.0 - m);
        let options = Prep {
            area: Area {
                region: DeviceRect {
                    x0: 0,
                    y0: 0,
                    x1: 1,
                    y1: 1,
                },
                band_height: 1,
            },
            limits: &limits,
            clip: None,
        };
        // Recover z=M(1-t) from x=0 using the original quadratic, then
        // integrate its pixel-local polynomial. This avoids H(tb)-H(ta)
        // and its large, nearly equal antiderivative values entirely.
        let u = origin.x + m;
        let z = 2.0 * u / (1.5 + (2.25 - 2.0 * u / m).sqrt());
        let exact = 1.0 - (u * z - 0.75 * z * z + z * z * z / (6.0 * m));
        assert!((exact - 0.664_705_581_461_998_8).abs() < 2e-15);
        assert_eq!(quantize_unit(exact), 169);
        let prepared = super::super::placed(&edges, origin, &options)
            .unwrap()
            .unwrap();
        let mut cells = Cells::new(1, &mut Meter::new(limits.render_bytes, RENDER_BYTES)).unwrap();
        raster::inspect(&prepared, &mut cells, |_, _, _, value, error| {
            assert!(
                (value - exact).abs() <= error,
                "primitive area {value} ±{error} excludes analytic {exact}"
            );
        })
        .unwrap();
        let mut work = Meter::new(limits.raster_work, RASTER_WORK);
        let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
        assert_eq!(
            ambiguous(&prepared, &Bound::Whole(0.0), (&mut work, &mut bytes))
                .unwrap()
                .len(),
            1,
        );
        match super::super::place(&edges, origin, &options) {
            Ok(Some(prepared)) => {
                raster::inspect(&prepared, &mut cells, |_, _, _, value, _| {
                    assert_eq!(quantize_unit(value), 169);
                })
                .unwrap();
            }
            Err(error) => {
                assert!(matches!(error.kind, pagura_core::ErrorKind::Limit));
                assert!(matches!(
                    error.message,
                    "unresolved curve coverage interval" | "curve subdivision depth exceeded"
                ));
            }
            Ok(None) => panic!("the mark covers the pixel"),
        }
    }

    #[test]
    fn replacement_releases_dead_builder_scratch_before_the_next_inspection() {
        let limits = Limits::default();
        let mut path = PathBuilder::new();
        let h = 3.0 * 0.500_001 / 255.0;
        for _ in 0..2 {
            path.move_to(Point::default());
            path.cubic_to(
                Point::new(1.0 / 3.0, 2.0 * h / 3.0),
                Point::new(2.0 / 3.0, 2.0 * h / 3.0),
                Point::new(1.0, 0.0),
            )
            .unwrap();
            path.close();
        }
        let edges = std::sync::Arc::new(
            GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap(),
        );
        let options = Prep {
            area: Area {
                region: DeviceRect {
                    x0: 0,
                    y0: 0,
                    x1: 1,
                    y1: 1,
                },
                band_height: 1,
            },
            limits: &limits,
            clip: None,
        };
        let shape = super::super::placed(&edges, Point::default(), &options)
            .unwrap()
            .unwrap();
        let mut work = Meter::new(limits.raster_work, RASTER_WORK);
        let source = Source::of(&edges, Point::default(), &mut work)
            .unwrap()
            .unwrap();
        let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
        bytes.charge(shape.retained_bytes()).unwrap();
        let shape = replacement(
            source,
            shape,
            &[[0.0, 0.0, 1.0, 1.0]],
            (&options, 0),
            Meters {
                work: &mut work,
                bytes: &mut bytes,
            },
        )
        .unwrap();
        assert_eq!(bytes.used(), shape.retained_bytes());
        let Body::Edges(body) = &shape.body else {
            panic!("replacement must be a polygon")
        };
        let mut needed = Meter::new(usize::MAX, RENDER_BYTES);
        Cells::charge(body.scan.1 - body.scan.0, &mut needed).unwrap();
        // One pending pixel must coexist with the cell buffer. Dead row
        // builder allocations must not consume this exact next-stage peak.
        let mut tight = Meter::new(
            bytes.used() + needed.used() + size_of::<Pixel>(),
            RENDER_BYTES,
        );
        tight.charge(bytes.used()).unwrap();
        let pending = ambiguous(&shape, &Bound::Whole(AREA), (&mut work, &mut tight)).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(tight.used(), shape.retained_bytes() + size_of::<Pixel>());
    }

    #[test]
    fn canonical_clip_interpolation_cancellation_stays_inside_the_interval() {
        let limits = Limits::default();
        let n = 2.0_f64.powi(39);
        let u = 2.0_f64.powi(-13);
        let mut clip = PathBuilder::new();
        clip.move_to(Point::new(-n, -n));
        clip.line_to(Point::new(n, n + u)).unwrap();
        clip.line_to(Point::new(-n, n + 1.0)).unwrap();
        clip.close();
        let clip = clip.finish();
        let area = Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 1,
            },
            band_height: 1,
        };
        let clip = ClipShape::new(
            &[(&clip, FillRule::NonZero)],
            Transform::IDENTITY,
            &ShapeOptions {
                area,
                limits: &limits,
            },
        )
        .unwrap();
        // This quadratic mark contains the whole unit pixel and has its
        // curved boundary outside it. Only the canonical clip can change
        // the pixel, so zero curve/localization error is not sufficient.
        let mut mark = PathBuilder::new();
        mark.move_to(Point::new(-2.0, -2.0));
        mark.cubic_to(
            Point::new(-2.0 / 3.0, -10.0 / 3.0),
            Point::new(2.0 / 3.0, -10.0 / 3.0),
            Point::new(2.0, -2.0),
        )
        .unwrap();
        mark.line_to(Point::new(2.0, 2.0)).unwrap();
        mark.line_to(Point::new(-2.0, 2.0)).unwrap();
        mark.close();
        let mark = mark.finish();
        let source = Source {
            outline: &mark,
            linear: Transform::IDENTITY,
            origin: Point::default(),
            points: &[],
        };
        let options = Prep {
            area,
            limits: &limits,
            clip: Some(&clip),
        };
        let mut work = Meter::new(limits.raster_work, RASTER_WORK);
        let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
        let (actual, error) = interval(
            source,
            Pixel {
                x: 0,
                y: 0,
                byte: 0,
            },
            (&options, 0),
            (&mut work, &mut bytes),
        )
        .unwrap();
        // The original stored line is x(y)=(y-u/2)/(1+u/(2N)). Integrate
        // from its x=0 crossing u/2 to y=1, independently of Line::x.
        let exact = (1.0 - u / 2.0).powi(2) / (2.0 * (1.0 + u / (2.0 * n)));
        assert_eq!(quantize_unit(exact), 127);
        assert!(
            (actual - exact).abs() <= error + 1e-15,
            "clip area{actual} ±{error} excludes analytic {exact}"
        );
        let edges =
            std::sync::Arc::new(GlyphEdges::new(&mark, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
        assert_eq!(
            super::super::place(&edges, Point::default(), &options)
                .unwrap_err()
                .message,
            "unresolved curve coverage interval",
        );
    }

    #[test]
    fn canonical_source_line_cancellation_cannot_bypass_curve_intervals() {
        let limits = Limits::default();
        let n = 2.0_f64.powi(39);
        let u = 2.0_f64.powi(-13);
        let mut path = PathBuilder::new();
        path.move_to(Point::new(-n, -n));
        path.line_to(Point::new(n, n + u)).unwrap();
        path.line_to(Point::new(-n, n + 1.0)).unwrap();
        path.close();
        for _ in 0..2 {
            path.move_to(Point::new(3.0, 3.0));
            path.cubic_to(
                Point::new(3.0 + 1.0 / 3.0, 3.0 + 2.0 / 3.0),
                Point::new(3.0 + 2.0 / 3.0, 3.0 + 2.0 / 3.0),
                Point::new(4.0, 3.0),
            )
            .unwrap();
            path.close();
        }
        let path = path.finish();
        let edges =
            std::sync::Arc::new(GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
        assert!(!edges.curved());
        let area = Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 1,
            },
            band_height: 1,
        };
        let options = Prep {
            area,
            limits: &limits,
            clip: None,
        };
        let source = Source {
            outline: &path,
            linear: Transform::IDENTITY,
            origin: Point::default(),
            points: &[],
        };
        let mut work = Meter::new(limits.raster_work, RASTER_WORK);
        let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
        let (actual, error) = interval(
            source,
            Pixel {
                x: 0,
                y: 0,
                byte: 0,
            },
            (&options, 0),
            (&mut work, &mut bytes),
        )
        .unwrap();
        let exact = (1.0 - u / 2.0).powi(2) / (2.0 * (1.0 + u / (2.0 * n)));
        assert_eq!(quantize_unit(exact), 127);
        assert!((actual - exact).abs() <= error + 1e-15);
        assert_eq!(
            super::super::place(&edges, Point::default(), &options)
                .unwrap_err()
                .message,
            "unresolved curve coverage interval",
        );
    }

    /// Small overlapping quadratic loops, several to a pixel, as a textured
    /// font draws them, wound both ways.
    fn dense() -> Path {
        let mut path = PathBuilder::new();
        for i in 0..12 {
            let (cx, cy) = (3.0 + 0.71 * f64::from(i % 4), 3.0 + 0.83 * f64::from(i / 4));
            let r = 0.6 + 0.13 * f64::from(i % 5);
            let sign = if i % 3 == 0 { -1.0 } else { 1.0 };
            let p = |x: f64, y: f64| Point::new(cx + x * r, cy + sign * y * r);
            path.move_to(p(1.0, 0.0));
            for [control, end] in [
                [(1.0, 1.0), (0.0, 1.0)],
                [(-1.0, 1.0), (-1.0, 0.0)],
                [(-1.0, -1.0), (0.0, -1.0)],
                [(1.0, -1.0), (1.0, 0.0)],
            ] {
                path.quad_to(p(control.0, control.1), p(end.0, end.1))
                    .unwrap();
            }
            path.close();
        }
        path.finish()
    }

    fn dense_options(limits: &Limits) -> Prep<'_> {
        Prep {
            area: Area {
                region: DeviceRect {
                    x0: 0,
                    y0: 0,
                    x1: 16,
                    y1: 16,
                },
                band_height: 16,
            },
            limits,
            clip: None,
        }
    }

    #[test]
    fn certified_dense_placements_emit_each_pixels_exact_byte() {
        let limits = Limits::default();
        let options = dense_options(&limits);
        let outline = dense();
        // Placements at 0.65 and 0.675 have wrong polygon bytes: they are
        // replaced before every byte certifies.
        let mut replaced = 0;
        for linear in [[1.0, 0.0, 0.0, 1.0], [1.5, 0.0, 0.25, 1.25]] {
            let edges = std::sync::Arc::new(GlyphEdges::new(&outline, linear, &limits).unwrap());
            assert!(!edges.curved());
            for fraction in [0.0, 0.3, 0.65, 0.675, 0.71] {
                let origin = Point::new(2.0 + fraction, 1.0 + (fraction * 7.0).fract());
                let shape = super::super::place(&edges, origin, &options)
                    .unwrap()
                    .unwrap();
                replaced += usize::from(!shape.resolved.is_empty());
                let (coverage, _) = crate::testing::raster(&options.area, Some(&shape), None);
                let mut work = Meter::new(limits.raster_work, RASTER_WORK);
                let mut bytes = crate::testing::bytes();
                let source = Source::of(&edges, origin, &mut work).unwrap().unwrap();
                for y in 0..16 {
                    for x in 0..16 {
                        let pixel = Pixel { x, y, byte: 0 };
                        let exact =
                            true_byte(source, pixel, &options, (&mut work, &mut bytes)).unwrap();
                        assert_eq!(coverage[(y * 16 + x) as usize], exact, "pixel {x},{y}");
                    }
                }
            }
        }
        assert_eq!(replaced, 3);
    }

    #[test]
    fn a_row_of_pixels_finds_the_bytes_each_pixel_finds_alone() {
        let limits = Limits::default();
        let options = dense_options(&limits);
        let outline = dense();
        let edges = GlyphEdges::new(&outline, [1.5, 0.0, 0.25, 1.25], &limits).unwrap();
        let origin = Point::new(2.3, 1.1);
        let mut work = Meter::new(limits.raster_work, RASTER_WORK);
        let mut bytes = crate::testing::bytes();
        let source = Source::of(&edges, origin, &mut work).unwrap().unwrap();
        let rows = Rows::new(source.outline, source.linear, slack(source), &mut bytes).unwrap();
        // Adjacent pixels, gaps between them, and pixels outside the glyph.
        let columns = [0, 3, 4, 5, 7, 9, 10, 13, 15];
        for y in 2..10 {
            let mut found = Vec::new();
            row_bytes(
                source,
                &rows,
                (y, &columns),
                &options,
                (&mut work, &mut bytes),
                &mut found,
            )
            .unwrap();
            for (&x, &byte) in columns.iter().zip(&found) {
                let pixel = Pixel { x, y, byte: 0 };
                let alone = true_byte(source, pixel, &options, (&mut work, &mut bytes)).unwrap();
                assert_eq!(byte, alone, "pixel {x},{y}");
            }
        }
    }
}
