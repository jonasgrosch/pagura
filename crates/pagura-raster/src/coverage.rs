//! Exact integration of winding-selected horizontal spans.
//!
//! Each pixel row is split at vertices and edge intersections. Within a slab,
//! edge order is fixed and span boundaries are affine in y. Integrating their
//! clamped ramps gives exact pixel areas, including self intersections and
//! overlapping contours. Unlike clamping a signed total area, this applies the
//! fill rule before integrating, so overlapping fractional shapes stay correct.
//! Scratch storage is per scanline/edge, never per pixel. Intersection discovery
//! is quadratic in active edges; an explicit work budget bounds hostile paths.

use crate::flatten::{MAX_SEGMENTS, bounded, flatten};
use crate::{FillRule, Path, Point, Transform};
use pagura_core::{Error, Result};
use std::sync::Arc;

const MAX_WORK: usize = 128_000_000;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shape {
    pub(crate) path: Arc<Path>,
    pub(crate) transform: Transform,
    pub(crate) rule: FillRule,
}
// Shapes only enter a successfully constructed Mask after finite geometry and
// transform validation. No NaN can occur in this private representation.
impl Eq for Shape {}

#[derive(Debug, Clone, Copy)]
struct Edge {
    a: Point,
    b: Point,
    winding: i32,
    group: usize,
    rule: FillRule,
}

impl Edge {
    fn new(a: Point, b: Point, group: usize, rule: FillRule) -> Option<Self> {
        if a.y == b.y {
            None
        } else if a.y < b.y {
            Some(Self {
                a,
                b,
                winding: 1,
                group,
                rule,
            })
        } else {
            Some(Self {
                a: b,
                b: a,
                winding: -1,
                group,
                rule,
            })
        }
    }

    fn x(self, y: f64) -> f64 {
        let t = ((y - self.a.y) / (self.b.y - self.a.y)).clamp(0.0, 1.0);
        self.a.x + t * (self.b.x - self.a.x)
    }
}

pub(crate) fn coverage(
    width: u32,
    height: u32,
    shapes: &[Shape],
    out: &mut [u8],
    origin: (u32, u32),
) -> Result<()> {
    let (edges, lower, upper) = edges(shapes, height, origin.1)?;
    let stride = usize::try_from(width).map_err(|_| Error::limit("mask width exceeds usize"))?;
    let begin = index(lower.floor() - f64::from(origin.1), height);
    let end = index(upper.ceil() - f64::from(origin.1), height);
    let mut active = Vec::new();
    let mut events = Vec::new();
    let mut ordered = Vec::new();
    let mut areas = Vec::new();
    areas
        .try_reserve_exact(stride)
        .map_err(|_| Error::limit("scanline allocation failed"))?;
    areas.resize(stride, 0.0);
    let mut work = 0;
    let mut windings = vec![0_i32; shapes.len()];
    out.fill(0);
    for (row, output) in out
        .chunks_exact_mut(stride)
        .enumerate()
        .take(end)
        .skip(begin)
    {
        charge(&mut work, edges.len())?;
        let y0 = f64::from(u32::try_from(row).map_err(|_| Error::limit("row exceeds u32"))?)
            + f64::from(origin.1);
        let y1 = y0 + 1.0;
        active.clear();
        events.clear();
        events.extend([y0, y1]);
        for &edge in &edges {
            if edge.a.y < y1 && edge.b.y > y0 {
                active.push(edge);
                if edge.a.y > y0 {
                    event(&mut events, edge.a.y)?;
                }
                if edge.b.y < y1 {
                    event(&mut events, edge.b.y)?;
                }
            }
        }
        intersections(&active, y0, y1, &mut events, &mut work)?;
        events.sort_unstable_by(f64::total_cmp);
        events.dedup();
        areas.fill(0.0);
        for slab in events.windows(2) {
            let [lo, hi] = [slab[0], slab[1]];
            if hi <= lo {
                continue;
            }
            let mid = lo + (hi - lo) * 0.5;
            ordered.clear();
            ordered.extend(
                active
                    .iter()
                    .filter(|e| e.a.y < mid && e.b.y > mid)
                    .map(|&e| (e.x(mid), e)),
            );
            ordered.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
            charge(&mut work, ordered.len())?;
            windings.fill(0);
            let mut outside = shapes.len();
            let mut left = None;
            for &(_, edge) in &ordered {
                let was_inside = outside == 0;
                let winding = windings
                    .get_mut(edge.group)
                    .ok_or(Error::invalid_argument("invalid winding group"))?;
                let previous = inside(*winding, edge.rule);
                *winding += edge.winding;
                let next = inside(*winding, edge.rule);
                if !previous && next {
                    outside -= 1;
                } else if previous && !next {
                    outside += 1;
                }
                let is_inside = outside == 0;
                if !was_inside && is_inside {
                    left = Some(edge);
                } else if was_inside
                    && !is_inside
                    && let Some(left) = left.take()
                {
                    span(
                        &mut areas,
                        [left, edge],
                        [lo, hi],
                        width,
                        origin.0,
                        &mut work,
                    )?;
                }
            }
        }
        for (pixel, area) in output.iter_mut().zip(&areas) {
            *pixel = quantize(*area);
        }
    }
    Ok(())
}

fn edges(shapes: &[Shape], height: u32, origin: u32) -> Result<(Vec<Edge>, f64, f64)> {
    let mut edges = Vec::new();
    let mut lower = f64::from(origin) + f64::from(height);
    let mut upper = 0.0_f64;
    for (group, shape) in shapes.iter().enumerate() {
        for contour in flatten(&shape.path, shape.transform)? {
            let mut points = contour.points.into_iter().map(|p| p * shape.transform);
            let Some(first) = points.next() else { continue };
            let mut previous = first;
            for next in points.chain(std::iter::once(first)) {
                bounded(next)?;
                if let Some(edge) = Edge::new(previous, next, group, shape.rule) {
                    if edges.len() >= MAX_SEGMENTS {
                        return Err(Error::limit("combined mask edge budget exceeded"));
                    }
                    lower = lower.min(edge.a.y);
                    upper = upper.max(edge.b.y);
                    edges.push(edge);
                }
                previous = next;
            }
        }
    }
    Ok((edges, lower, upper))
}

fn intersections(
    active: &[Edge],
    y0: f64,
    y1: f64,
    events: &mut Vec<f64>,
    work: &mut usize,
) -> Result<()> {
    // Segment intersections split the row before winding is evaluated.
    // Parallel/coincident edges need no event: they are processed together.
    for (i, a) in active.iter().enumerate() {
        for b in active.iter().skip(i + 1) {
            charge(work, 1)?;
            let lo = y0.max(a.a.y).max(b.a.y);
            let hi = y1.min(a.b.y).min(b.b.y);
            if lo >= hi {
                continue;
            }
            let d0 = a.x(lo) - b.x(lo);
            let d1 = a.x(hi) - b.x(hi);
            if (d0 < 0.0 && d1 > 0.0) || (d0 > 0.0 && d1 < 0.0) {
                event(events, lo + (hi - lo) * (d0 / (d0 - d1)))?;
            }
        }
    }
    Ok(())
}

fn event(events: &mut Vec<f64>, y: f64) -> Result<()> {
    if events.len() >= MAX_SEGMENTS {
        return Err(Error::limit("scanline event budget exceeded"));
    }
    events.push(y);
    Ok(())
}

fn inside(winding: i32, rule: FillRule) -> bool {
    match rule {
        FillRule::NonZero => winding != 0,
        FillRule::EvenOdd => winding % 2 != 0,
    }
}

fn span(
    areas: &mut [f64],
    edges: [Edge; 2],
    interval: [f64; 2],
    width: u32,
    origin: u32,
    work: &mut usize,
) -> Result<()> {
    let [left, right] = edges;
    let [lo, hi] = interval;
    let l = [left.x(lo), left.x(hi)];
    let r = [right.x(lo), right.x(hi)];
    let start = index(l[0].min(l[1]).floor() - f64::from(origin), width);
    let end = index(r[0].max(r[1]).ceil() - f64::from(origin), width);
    charge(work, end.saturating_sub(start))?;
    for (x, area) in areas.iter_mut().enumerate().take(end).skip(start) {
        let x = f64::from(u32::try_from(x).map_err(|_| Error::limit("column exceeds u32"))?)
            + f64::from(origin);
        *area += (hi - lo) * (ramp(r[0] - x, r[1] - x) - ramp(l[0] - x, l[1] - x));
    }
    Ok(())
}

// Integral on [0,1] of clamp(a + (b-a)t, 0, 1). Splitting at the two
// clamp boundaries avoids cancellation of large squared coordinates.
fn ramp(a: f64, b: f64) -> f64 {
    let (a, b) = (a.min(b), a.max(b));
    if a >= 1.0 {
        return 1.0;
    }
    if b <= 0.0 {
        return 0.0;
    }
    if a == b {
        return a.clamp(0.0, 1.0);
    }
    let start = ((0.0 - a) / (b - a)).clamp(0.0, 1.0);
    let end = ((1.0 - a) / (b - a)).clamp(0.0, 1.0);
    let low = a.max(0.0);
    let high = b.min(1.0);
    (end - start) * (low + high) * 0.5 + (1.0 - end)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to the u32 surface extent and converted to an index"
)]
fn index(value: f64, max: u32) -> usize {
    value.clamp(0.0, f64::from(max)) as usize
}

pub(crate) fn charge(work: &mut usize, amount: usize) -> Result<()> {
    *work = work
        .checked_add(amount)
        .filter(|&n| n <= MAX_WORK)
        .ok_or(Error::limit("raster work budget exceeded"))?;
    Ok(())
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "coverage is rounded after clamping to 0..=255"
)]
fn quantize(area: f64) -> u8 {
    (area.clamp(0.0, 1.0) * 255.0).round() as u8
}
