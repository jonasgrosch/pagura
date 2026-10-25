//! Overlapping contours resolved once per outline and linear map.
//!
//! A polygon whose winding certificate leaves slabs in doubt is placed as
//! the sides of its covered set: the rows of the outline's own grid that
//! meet a doubtful slab take the row resolver's sides, and every other
//! height keeps the pieces of the polygon's edges, whose winding the
//! certificate leaves 0 or the common sign. A placement then adds its
//! origin to these sides as to a certified spline's edges, instead of
//! translating the polygon, sweeping it and resolving its rows again.
//!
//! Every such placement certifies its coverage bytes against the original
//! outline (ADR 0004 §4): a byte the area bound leaves certain is the byte
//! of the exact covered area, and every other byte is refined until it is.
//! So the bytes are those of the exact area, whichever sides approximate it
//! within the bound.

use super::{Relative, rows_reached};
use crate::band::RASTER_WORK;
use crate::band::certify::Doubtful;
use crate::band::edges::{self, Line, Polygon};
use crate::band::union::{Meters, Piece, Resolver, room};
use crate::{FillRule, Point};
use pagura_core::{Error, Limits, Meter, Result};

/// The sides of `polygon`'s covered set relative to the glyph origin, with
/// only their storage left charged to `bytes`. `None` beyond a limit or
/// without sides: each placement then resolves its own rows.
pub(super) fn resolved(
    polygon: &Polygon,
    doubtful: &Doubtful,
    limits: &Limits,
    bytes: &mut Meter,
) -> Option<Box<Relative>> {
    let before = bytes.used();
    let result = sides(polygon, doubtful, limits, bytes);
    let retained = result
        .as_ref()
        .map_or(0, |lines| lines.capacity() * size_of::<Line>());
    bytes.refund(bytes.used().saturating_sub(before.saturating_add(retained)));
    match result {
        Ok(lines) if !lines.is_empty() && bytes.charge(size_of::<Relative>()).is_ok() => {
            Some(Box::new(relative(lines)))
        }
        _ => {
            bytes.refund(retained);
            None
        }
    }
}

fn sides(
    polygon: &Polygon,
    doubtful: &Doubtful,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<Vec<Line>> {
    let mut work = Meter::new(limits.raster_work, RASTER_WORK);
    let lines = edges::lines(polygon, bytes)?;
    // The rows `k..k + 1` that meet a slab, merged; the slabs ascend.
    let mut ranges: Vec<(f64, f64)> = Vec::new();
    for &[lo, hi] in &doubtful.slabs {
        let (first, end) = (lo.floor(), hi.ceil());
        match ranges.last_mut() {
            Some(last) if last.1 >= first => last.1 = last.1.max(end),
            _ => {
                room(&mut ranges, 1, bytes)?;
                ranges.push((first, end));
            }
        }
    }
    let mut out = Vec::new();
    room(&mut out, lines.len(), bytes)?;
    work.charge_product(lines.len(), ranges.len().saturating_add(1))?;
    for &line in &lines {
        kept(line, &ranges, &mut out, bytes)?;
    }
    let resolved = out.len();
    let mut resolver = Resolver::default();
    let mut pieces = Vec::new();
    for &(first, end) in &ranges {
        let mut top = first;
        while top < end {
            pieces.clear();
            room(&mut pieces, lines.len(), bytes)?;
            pieces.extend(lines.iter().filter_map(|&line| Piece::new(line, top, 0)));
            work.charge(lines.len().saturating_add(pieces.len()))?;
            let meters = Meters {
                work: &mut work,
                bytes,
            };
            resolver.row(&pieces, &[FillRule::NonZero], top, (&mut out, meters))?;
            top += 1.0;
        }
    }
    // The resolver's sides wind 1. Where the kept pieces wind -1, its sides
    // turn, so that a pixel taking both never cancels them.
    if !doubtful.positive {
        for side in &mut out[resolved..] {
            *side = Line {
                a: side.b,
                b: side.a,
            };
        }
    }
    out.retain(|line| !line.is_horizontal());
    // Retained in the store like the polygon, within the same budget.
    if out.len() > limits.path_segments {
        return Err(Error::limit("flattened segment budget exceeded"));
    }
    Ok(out)
}

/// Appends the pieces of `line` outside the row `ranges`, in its direction,
/// each end on the line where a range cuts it.
fn kept(line: Line, ranges: &[(f64, f64)], out: &mut Vec<Line>, bytes: &mut Meter) -> Result<()> {
    let down = line.a.y < line.b.y;
    let (top, bottom) = if down {
        (line.a.y, line.b.y)
    } else {
        (line.b.y, line.a.y)
    };
    let mut keep = |from: f64, to: f64| -> Result<()> {
        if from < to {
            let (a, b) = (Point::new(line.x(from), from), Point::new(line.x(to), to));
            room(out, 1, bytes)?;
            out.push(if down {
                Line { a, b }
            } else {
                Line { a: b, b: a }
            });
        }
        Ok(())
    };
    let mut from = top;
    for &(first, end) in ranges {
        if first >= bottom {
            break;
        }
        keep(from, first)?;
        from = from.max(end);
    }
    keep(from, bottom)
}

/// Sides as a placement translates them, with the extent and work a
/// certified spline's edges carry.
fn relative(lines: Vec<Line>) -> Relative {
    let mut lo = Point::new(f64::INFINITY, f64::INFINITY);
    let mut hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut work = 0_usize;
    for &line in &lines {
        for p in [line.a, line.b] {
            lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        let rows = rows_reached((line.a.y.min(line.b.y), line.a.y.max(line.b.y)));
        work = work.saturating_add(edges::line_work(line, rows, usize::MAX).saturating_mul(4));
    }
    Relative {
        lines,
        curves: Vec::new(),
        hull: [lo, hi],
        work,
    }
}
