//! An optional complete coverage owner after the original byte collector.
//!
//! Only an invocation's first, otherwise discarded inspection allowance can
//! pay this replay. The parent meters and geometry remain untouched on every
//! decline. No source proof or byte correction survives into execution.

use super::{Pixel, Source, support_contained};
use crate::band::cells::Cells;
use crate::band::coverage::CertifiedRows;
use crate::band::edges::{self, Bounds, Line};
use crate::band::shape::{Frame, Shape};
use crate::band::union::Meters;
use crate::band::{Prep, RASTER_WORK, RENDER_BYTES, raster};
use crate::flatten::MAX_COORDINATE;
use crate::{Point, Segment, quantize_unit};
use pagura_core::{Error, Meter, Result};

/// Storage and execution work every successful full replacement must retain
/// or spend for the counted nonhorizontal original endpoint pairs.
pub(super) struct Floor {
    pub(super) bytes: usize,
    pub(super) work: usize,
}

/// Source-space subdivision enclosure and an actually retained line witness.
pub(super) struct Witness {
    user: Bounds,
    placed: Bounds,
    lines: usize,
}

pub(super) fn replace(
    source: Source<'_>,
    shape: Shape,
    options: &Prep<'_>,
    proven: &[Pixel],
    surplus: Option<usize>,
    bytes: &mut Meter,
) -> Shape {
    let Some(rows) = surplus
        .and_then(|surplus| prepare(source, &shape, options, proven, surplus, bytes.remaining()))
    else {
        return shape;
    };
    if bytes.charge(rows.heap_bytes()).is_err() {
        return shape;
    }
    // Transfer the child owner while old geometry remains charged. Publication
    // allocates nothing; refund only after its old body/Resolved owners drop.
    let released = shape.retained_bytes().saturating_sub(size_of::<Shape>());
    let ready = shape.with_certified_rows(rows);
    bytes.refund(released);
    ready
}

pub(super) fn prepare(
    source: Source<'_>,
    shape: &Shape,
    options: &Prep<'_>,
    proven: &[Pixel],
    surplus: usize,
    remaining_bytes: usize,
) -> Option<CertifiedRows> {
    let mut work = Meter::new(surplus, RASTER_WORK);
    let mut bytes = Meter::new(remaining_bytes, RENDER_BYTES);
    attempt(
        source,
        shape,
        options,
        proven,
        surplus,
        Meters {
            work: &mut work,
            bytes: &mut bytes,
        },
    )
    .ok()
    .flatten()
}

pub(super) fn attempt(
    source: Source<'_>,
    shape: &Shape,
    options: &Prep<'_>,
    proven: &[Pixel],
    surplus: usize,
    Meters { work, bytes }: Meters<'_>,
) -> Result<Option<CertifiedRows>> {
    work.charge(16)?;
    let Some(floor) = floor(source, shape, options, work)? else {
        return Ok(None);
    };
    // The same immutable shape and fixed-size Cells repeat the exact I+16
    // tally. Pay it from S before allocating or entering the common kernel.
    let replay = shape
        .work()
        .checked_sub(surplus)
        .ok_or(Error::limit(RASTER_WORK))?;
    work.charge(replay)?;
    let width = super::scan_width(shape);
    // The consumed inspection tally starts after Cells construction. This
    // extra replay must also pay every initialized byte of its four buffers.
    let mut initialized = Meter::new(usize::MAX, RENDER_BYTES);
    Cells::charge(width, &mut initialized)?;
    work.charge(initialized.used())?;
    let mut cells = Cells::new(width, bytes)?;
    let cell_bytes = cells.retained_bytes();
    let mut encoded = Encoding {
        rows: CertifiedRows::new(),
        proven,
        next: 0,
        failure: None,
    };
    let observed = raster::inspect(shape, &mut cells, |y, x0, x1, area, _| {
        if encoded.failure.is_none()
            && let Err(error) = encoded.range(y, (x0, x1), area, Meters { work, bytes })
        {
            // A failed callback cannot interrupt or shorten the prepaid
            // numerical traversal. It only stops optional encoding.
            encoded.failure = Some(error);
        }
    });
    drop(cells);
    bytes.refund(cell_bytes);
    let observed = observed?;
    work.charge(8)?;
    if encoded.failure.is_some()
        || observed.surplus(shape.work()) != Some(surplus)
        || encoded.next != proven.len()
    {
        return Ok(None);
    }
    encoded.rows.finish(&options.area, work)?;
    work.charge(4)?;
    if encoded.rows.bounds() != Some(shape.bounds()) || encoded.rows.work() > floor.work {
        return Ok(None);
    }
    Ok(encoded
        .rows
        .fit_heap(floor.bytes, work, bytes)?
        .then_some(encoded.rows))
}

/// Sorted original-control successes replace every ambiguous byte during the
/// common replay. All other bytes already passed the initial interval proof.
struct Encoding<'a> {
    rows: CertifiedRows,
    proven: &'a [Pixel],
    next: usize,
    failure: Option<Error>,
}

impl Encoding<'_> {
    fn range(
        &mut self,
        y: u32,
        (x0, x1): (usize, usize),
        area: f64,
        Meters { work, bytes }: Meters<'_>,
    ) -> Result<()> {
        work.charge(4)?;
        if !area.is_finite() {
            return Err(Error::limit("nonfinite certified row"));
        }
        let column = |x| u32::try_from(x).map_err(|_| Error::limit("glyph column exceeds u32"));
        let (mut x, end) = (column(x0)?, column(x1)?);
        let value = quantize_unit(area);
        while let Some(pixel) = self.proven.get(self.next) {
            work.charge(2)?;
            if (pixel.y, pixel.x) < (y, x) {
                return Err(Error::limit("incomplete certified row replay"));
            }
            if pixel.y != y || pixel.x >= end {
                break;
            }
            self.rows.push_range(y, x, pixel.x, value, work, bytes)?;
            x = pixel
                .x
                .checked_add(1)
                .ok_or(Error::limit("glyph column exceeds u32"))?;
            self.rows
                .push_range(y, pixel.x, x, pixel.byte, work, bytes)?;
            self.next += 1;
        }
        self.rows.push_range(y, x, end, value, work, bytes)
    }
}

pub(super) fn floor(
    source: Source<'_>,
    shape: &Shape,
    options: &Prep<'_>,
    work: &mut Meter,
) -> Result<Option<Floor>> {
    // In particular, a tight/analytic initial shape must not rely on its
    // guard_candidate shortcut to claim zero source coverage outside bounds.
    if !support_contained(source, Some(shape), options, work)? {
        return Ok(None);
    }
    let Some(frame) = Frame::new(options).filter(|frame| frame.paths.is_none()) else {
        return Ok(None);
    };
    let Some(witness) = witness(source, work)? else {
        return Ok(None);
    };
    let body = emitted_bounds(source, witness.user, work)?;
    let r = options.area.region;
    // Strict right containment makes every counted line reach line_rows;
    // the closed Y range is wholly inside the request. The real rectangle
    // also keeps every emitted vertex and literal canonical point cell.
    if body.left < f64::from(r.x0)
        || body.top < f64::from(r.y0)
        || body.right >= f64::from(r.x1)
        || body.bottom > f64::from(r.y1)
        || frame.rect.is_some_and(|rect| !rect.contains(body))
        || edges::pixels(
            [
                Point::new(witness.placed.left, witness.placed.top),
                Point::new(witness.placed.right, witness.placed.bottom),
            ],
            frame.limit,
            false,
        )
        .is_empty()
    {
        return Ok(None);
    }
    let bounds = shape.bounds();
    let width = bounds.width() as usize;
    let overhead = width
        .checked_add(width / 64)
        .and_then(|row| row.checked_add(6))
        .and_then(|row| row.checked_mul(bounds.height() as usize));
    work.charge(4)?;
    // The final corrected nonzero bbox must equal these bounds. Any accepted
    // full replacement therefore has at least this height and scan width;
    // Shape::edges reserves this row overhead besides its 4*line_work.
    let work = witness
        .lines
        .checked_mul(24)
        .and_then(|lines| lines.checked_add(overhead?));
    // A containing replacement spans at least these global strips. When it
    // spans more than one, Shape::edges must retain a StripList: every counted
    // raw line has an entry, besides the strips+1 offsets. Resolved coverage
    // does not remove those raw entries. Single-strip shapes need no index.
    let Some(last) = bounds.y1.checked_sub(1).filter(|&last| last >= bounds.y0) else {
        return Ok(None);
    };
    let strips = edges::strips((bounds.y0, last));
    let index = if strips > 1 {
        witness
            .lines
            .checked_add(strips)
            .and_then(|n| n.checked_add(1))
            .and_then(|n| n.checked_mul(size_of::<u32>()))
    } else {
        Some(0)
    };
    Ok(witness
        .lines
        .checked_mul(size_of::<Line>())
        .and_then(|bytes| bytes.checked_add(index?))
        .zip(work)
        .map(|(bytes, work)| Floor { bytes, work }))
}

pub(super) fn witness(source: Source<'_>, work: &mut Meter) -> Result<Option<Witness>> {
    let mut user = empty_bounds();
    let mut placed = empty_bounds();
    let mut current = None;
    let mut start = Point::default();
    let mut lines = 0_usize;
    for segment in source.outline.segments() {
        // Each packet contains at most three source controls and two literal
        // endpoint maps; no subdivision or input-dependent inner loop.
        work.charge(16)?;
        let end = match *segment {
            Segment::MoveTo(point) => {
                include(&mut user, point);
                start = point;
                current = Some(point);
                continue;
            }
            Segment::LineTo(point) => {
                include(&mut user, point);
                point
            }
            Segment::CubicTo { c1, c2, end } => {
                for point in [c1, c2, end] {
                    include(&mut user, point);
                }
                end
            }
            Segment::Close => {
                // Closing pairs are deliberately omitted from the floor.
                current = Some(start);
                continue;
            }
        };
        let Some(previous) = current else {
            return Ok(None);
        };
        let a = previous * source.linear + source.origin;
        let b = end * source.linear + source.origin;
        if !a.is_finite() || !b.is_finite() {
            return Ok(None);
        }
        if a.y != b.y {
            lines = lines.checked_add(1).ok_or(Error::limit(RASTER_WORK))?;
            include(&mut placed, a);
            include(&mut placed, b);
        }
        current = Some(end);
    }
    Ok(
        (lines != 0 && finite(user) && finite(placed)).then_some(Witness {
            user,
            placed,
            lines,
        }),
    )
}

fn emitted_bounds(source: Source<'_>, user: Bounds, work: &mut Meter) -> Result<Bounds> {
    work.charge(16)?;
    let mut bounds = empty_bounds();
    // Literal finite midpoints stay within the original user-coordinate box.
    // Each affine coordinate is monotone in each input, including its rounded
    // products/additions; mapping the four corners encloses every emitted
    // vertex for every future subdivision depth.
    for x in [user.left, user.right] {
        for y in [user.top, user.bottom] {
            let point = Point::new(x, y) * source.linear + source.origin;
            if !point.is_finite() {
                return Err(Error::limit("certified row geometry exceeds range"));
            }
            include(&mut bounds, point);
        }
    }
    for &point in source.points {
        work.charge(4)?;
        let [a, _, b, _] = edges::Polygon::cell_corners(point + source.origin);
        if !a.is_finite() || !b.is_finite() {
            return Err(Error::limit("certified row geometry exceeds range"));
        }
        include(&mut bounds, a);
        include(&mut bounds, b);
    }
    if !finite(bounds)
        || [bounds.left, bounds.top, bounds.right, bounds.bottom]
            .into_iter()
            .any(|coordinate| coordinate.abs() > MAX_COORDINATE)
    {
        return Err(Error::limit("certified row geometry exceeds range"));
    }
    Ok(bounds)
}

fn empty_bounds() -> Bounds {
    Bounds {
        left: f64::INFINITY,
        top: f64::INFINITY,
        right: f64::NEG_INFINITY,
        bottom: f64::NEG_INFINITY,
    }
}

fn include(bounds: &mut Bounds, point: Point) {
    bounds.left = bounds.left.min(point.x);
    bounds.top = bounds.top.min(point.y);
    bounds.right = bounds.right.max(point.x);
    bounds.bottom = bounds.bottom.max(point.y);
}

fn finite(bounds: Bounds) -> bool {
    [bounds.left, bounds.top, bounds.right, bounds.bottom]
        .into_iter()
        .all(f64::is_finite)
}
