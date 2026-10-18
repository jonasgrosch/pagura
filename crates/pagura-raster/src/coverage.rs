//! Exact-area scanlines with signed cell areas and cover differences.
//!
//! Certified single spans accumulate directly. Other active edges select winding
//! boundaries before integration; endpoint-order inversions isolate crossing
//! blocks without an all-pairs scan. Partial-cell areas and prefix cover
//! differences resolve in one scan over the row. Horizontal clipping projects
//! outside geometry onto viewport boundaries, preserving winding and bounding
//! work by the surface.

use crate::flatten::{bounded, flatten};
use crate::{FillRule, Path, Point, Segment, Transform};
use pagura_core::{Error, Limits, Result};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shape {
    pub(crate) path: Arc<Path>,
    pub(crate) transform: Transform,
    pub(crate) rule: FillRule,
}
impl Shape {
    pub(crate) fn covers(&self, origin: (u32, u32), size: (u32, u32)) -> bool {
        let [
            Segment::MoveTo(a),
            Segment::LineTo(b),
            Segment::LineTo(c),
            Segment::LineTo(d),
            Segment::Close,
        ] = self.path.segments()
        else {
            return false;
        };
        let [a, b, c, d] = [*a, *b, *c, *d].map(|p| p * self.transform);
        let axis = (a.x == b.x && b.y == c.y && c.x == d.x && d.y == a.y)
            || (a.y == b.y && b.x == c.x && c.y == d.y && d.x == a.x);
        if !axis || a == b || b == c || c == d || d == a {
            return false;
        }
        let left = a.x.min(c.x);
        let right = a.x.max(c.x);
        let top = a.y.min(c.y);
        let bottom = a.y.max(c.y);
        left <= f64::from(origin.0)
            && top <= f64::from(origin.1)
            && right >= f64::from(origin.0) + f64::from(size.0)
            && bottom >= f64::from(origin.1) + f64::from(size.1)
    }
}
// Successfully constructed masks contain finite geometry only.
impl Eq for Shape {}

#[derive(Debug, Clone, Copy)]
struct Edge {
    a: Point,
    b: Point,
    winding: i64,
    group: usize,
    rule: FillRule,
    id: usize,
}

impl Edge {
    fn x(self, y: f64) -> f64 {
        let t = ((y - self.a.y) / (self.b.y - self.a.y)).clamp(0.0, 1.0);
        self.a.x + t * (self.b.x - self.a.x)
    }
    fn slope(self) -> f64 {
        (self.b.x - self.a.x) / (self.b.y - self.a.y)
    }
}

#[derive(Clone)]
struct Windings {
    values: Vec<i64>,
    outside: usize,
}
impl Windings {
    fn new(count: usize) -> Self {
        Self {
            values: vec![0; count],
            outside: count,
        }
    }
    fn reset(&mut self) {
        self.values.fill(0);
        self.outside = self.values.len();
    }
    // Closing a span contributes +area; opening contributes -area.
    fn step(&mut self, edge: Edge) -> Result<i32> {
        let was_inside = self.outside == 0;
        let value = self
            .values
            .get_mut(edge.group)
            .ok_or(Error::invalid_argument("invalid winding group"))?;
        let before = inside(*value, edge.rule);
        *value = value
            .checked_add(edge.winding)
            .ok_or(Error::limit("winding number overflow"))?;
        let after = inside(*value, edge.rule);
        if !before && after {
            self.outside -= 1;
        } else if before && !after {
            self.outside += 1;
        }
        Ok(i32::from(was_inside) - i32::from(self.outside == 0))
    }
}

struct Row {
    areas: Vec<f64>,
    covers: Vec<f64>,
    origin: u32,
    work: usize,
    maximum: usize,
}
impl Row {
    fn new(width: usize, origin: u32, maximum: usize) -> Result<Self> {
        let mut areas = Vec::new();
        areas
            .try_reserve_exact(width)
            .map_err(|_| Error::limit("scanline allocation failed"))?;
        areas.resize(width, 0.0);
        let count = width
            .checked_add(1)
            .ok_or(Error::limit("scanline width overflow"))?;
        let mut covers = Vec::new();
        covers
            .try_reserve_exact(count)
            .map_err(|_| Error::limit("scanline allocation failed"))?;
        covers.resize(count, 0.0);
        Ok(Self {
            areas,
            covers,
            origin,
            work: 0,
            maximum,
        })
    }
    fn charge(&mut self, amount: usize) -> Result<()> {
        charge(&mut self.work, amount, self.maximum)
    }
    fn sorted(&mut self, count: usize) -> Result<()> {
        self.charge(
            count
                .checked_mul(count.max(1).ilog2() as usize + 1)
                .ok_or(Error::limit("sort work overflow"))?,
        )
    }
    fn edge(&mut self, edge: Edge, lo: f64, hi: f64, sign: i32) -> Result<()> {
        if sign == 0 {
            return Ok(());
        }
        let a = edge.x(lo) - f64::from(self.origin);
        let b = edge.x(hi) - f64::from(self.origin);
        let start = index(a.min(b).floor(), self.areas.len());
        let end = index(a.max(b).ceil(), self.areas.len());
        let area = (hi - lo) * f64::from(sign);
        self.charge(end.saturating_sub(start) + 1)?;
        if start > 0 {
            if let Some(first) = self.covers.first_mut() {
                *first += area;
            }
            if let Some(after) = self.covers.get_mut(start) {
                *after -= area;
            }
        }
        for (x, target) in self.areas.iter_mut().enumerate().take(end).skip(start) {
            let x = f64::from(u32::try_from(x).map_err(|_| Error::limit("column exceeds u32"))?);
            *target += area * ramp(a - x, b - x);
        }
        Ok(())
    }
    fn finish(&self, output: &mut [u8]) {
        let mut cover = 0.0;
        for ((out, area), delta) in output.iter_mut().zip(&self.areas).zip(&self.covers) {
            cover += delta;
            *out = quantize(area + cover);
        }
    }
}

pub(crate) fn coverage(
    width: u32,
    height: u32,
    shapes: &[Shape],
    out: &mut [u8],
    origin: (u32, u32),
    limits: &Limits,
) -> Result<()> {
    let stride = usize::try_from(width).map_err(|_| Error::limit("mask width exceeds usize"))?;
    let mut row = Row::new(stride, origin.0, limits.raster_work)?;
    let mut edges = edges(shapes, (width, height), origin, limits, &mut row)?;
    // Edges have already validated the geometry and its budgets. A rectangular
    // intersection containing the whole viewport has exactly unit area in every
    // cell; no scanline arithmetic or sorting is needed.
    if !shapes.is_empty() && shapes.iter().all(|s| s.covers(origin, (width, height))) {
        row.charge(out.len())?;
        out.fill(255);
        return Ok(());
    }
    row.sorted(edges.len())?;
    edges.sort_unstable_by(|a, b| a.a.y.total_cmp(&b.a.y));
    let mut active: Vec<Edge> = Vec::new();
    let mut next = 0;
    let mut vertices = Vec::new();
    let mut selected: Vec<Edge> = Vec::new();
    let mut winding = Windings::new(shapes.len());
    out.fill(0);
    for (y, output) in out.chunks_exact_mut(stride).enumerate() {
        let lo = f64::from(u32::try_from(y).map_err(|_| Error::limit("row exceeds u32"))?)
            + f64::from(origin.1);
        let hi = lo + 1.0;
        active.retain(|e| e.b.y > lo);
        while let Some(&edge) = edges.get(next)
            && edge.a.y < hi
        {
            active.push(edge);
            next += 1;
        }
        if active.is_empty() {
            continue;
        }
        row.charge(active.len())?;
        row.areas.fill(0.0);
        row.covers.fill(0.0);
        if shapes.len() != 1 || !unit_span(&active, lo, hi, &mut row)? {
            vertices.clear();
            vertices.extend([lo, hi]);
            vertices.extend(
                active
                    .iter()
                    .flat_map(|e| [e.a.y, e.b.y])
                    .filter(|&y| y > lo && y < hi),
            );
            row.sorted(vertices.len())?;
            vertices.sort_unstable_by(f64::total_cmp);
            vertices.dedup();
            selected.clear();
            let mut admitted = 0;
            for interval in vertices.windows(2) {
                let mid = interval[0].midpoint(interval[1]);
                selected.retain(|e| e.b.y > mid);
                // Row edges retain their starting-y order. Admit each just once,
                // rather than searching the entire row at every vertex interval.
                while let Some(&edge) = active.get(admitted)
                    && edge.a.y < mid
                {
                    if edge.b.y > mid {
                        selected.push(edge);
                    }
                    admitted += 1;
                }
                strip(
                    &mut selected,
                    interval[0],
                    interval[1],
                    &mut winding,
                    &mut row,
                    limits,
                )?;
            }
        }
        row.charge(stride)?;
        row.finish(output);
    }
    Ok(())
}

// Opposite unit windings with a shared vertical extent and no crossing enclose
// one span. Both fill rules agree: signed area equals covered area in every
// cell. Global coordinates preserve tile coverage even if clipping changes
// which rows qualify.
fn unit_span(active: &[Edge], lo: f64, hi: f64, row: &mut Row) -> Result<bool> {
    let [a, b] = active else { return Ok(false) };
    if !matches!(a.winding, -1 | 1) || b.winding != -a.winding {
        return Ok(false);
    }
    if lo.max(a.a.y) != lo.max(b.a.y) || hi.min(a.b.y) != hi.min(b.b.y) {
        return Ok(false);
    }
    let (lo, hi) = (lo.max(a.a.y), hi.min(a.b.y));
    let (a, b) = if endpoint(*a, *b, lo, false).is_le() {
        (*a, *b)
    } else {
        (*b, *a)
    };
    if a.x(hi) > b.x(hi) {
        return Ok(false);
    }
    row.edge(a, lo, hi, -1)?;
    row.edge(b, lo, hi, 1)?;
    Ok(true)
}

fn strip(
    top: &mut [Edge],
    lo: f64,
    hi: f64,
    winding: &mut Windings,
    row: &mut Row,
    limits: &Limits,
) -> Result<()> {
    winding.reset();
    row.sorted(top.len())?;
    top.sort_unstable_by(|a, b| endpoint(*a, *b, lo, false));
    if top
        .windows(2)
        // Equal x at hi is a meeting, not a crossing inside
        // this strip. The order at lo already gives the interior order; slope
        // tie-breaking here would send ordinary closing vertices through the
        // exact crossing integrator unnecessarily.
        .all(|pair| pair[0].x(hi).total_cmp(&pair[1].x(hi)).is_le())
    {
        for &edge in top.iter() {
            row.edge(edge, lo, hi, winding.step(edge)?)?;
        }
        return Ok(());
    }
    let mut bottom: Vec<_> = top.iter().copied().enumerate().collect();
    row.sorted(bottom.len())?;
    bottom.sort_unstable_by(|(_, a), (_, b)| endpoint(*a, *b, hi, true));
    let mut ranks = vec![0; top.len()];
    for (rank, &(index, _)) in bottom.iter().enumerate() {
        *ranks
            .get_mut(index)
            .ok_or(Error::invalid_argument("invalid edge rank"))? = rank;
    }
    let mut start = 0;
    let mut end = 0;
    for (i, &rank) in ranks.iter().enumerate() {
        end = end.max(rank);
        if end != i {
            continue;
        }
        let block = top
            .get(start..=i)
            .ok_or(Error::invalid_argument("invalid crossing block"))?;
        if let [edge] = block {
            row.edge(*edge, lo, hi, winding.step(*edge)?)?;
        } else {
            crossing(block, lo, hi, winding, row, limits)?;
            for &edge in block {
                winding.step(edge)?;
            }
        }
        start = i + 1;
    }
    Ok(())
}

fn endpoint(a: Edge, b: Edge, y: f64, bottom: bool) -> std::cmp::Ordering {
    a.x(y)
        .total_cmp(&b.x(y))
        .then_with(|| {
            if bottom {
                b.slope().total_cmp(&a.slope())
            } else {
                a.slope().total_cmp(&b.slope())
            }
        })
        .then(a.id.cmp(&b.id))
}

fn crossing(
    block: &[Edge],
    lo: f64,
    hi: f64,
    initial: &Windings,
    row: &mut Row,
    limits: &Limits,
) -> Result<()> {
    let mut events = vec![lo, hi];
    let mut ordered: Vec<_> = block.iter().map(|&edge| (edge, edge.x(hi))).collect();
    // Insertion sorting top order by bottom x visits each strict inversion
    // once: O(edges + crossings), without merge buffers. Equal bottom positions
    // meet at the boundary and need no interior crossing event.
    for i in 1..ordered.len() {
        let mut j = i;
        while j > 0 {
            row.charge(1)?;
            let (&(a, ax), &(b, bx)) = ordered
                .get(j - 1)
                .zip(ordered.get(j))
                .ok_or(Error::invalid_argument("invalid crossing pair"))?;
            let d1 = ax - bx;
            if d1 <= 0.0 {
                break;
            }
            let d0 = a.x(lo) - b.x(lo);
            if d0 < 0.0 {
                if events.len() >= limits.path_segments {
                    return Err(Error::limit("crossing event budget exceeded"));
                }
                events.push(lo + (hi - lo) * (d0 / (d0 - d1)));
            }
            ordered.swap(j - 1, j);
            j -= 1;
        }
    }
    row.sorted(events.len())?;
    events.sort_unstable_by(f64::total_cmp);
    events.dedup();
    for interval in events.windows(2) {
        let (lo, hi) = (interval[0], interval[1]);
        row.sorted(ordered.len())?;
        // Evaluate each edge once per interval, rather than at every comparison.
        let mid = lo.midpoint(hi);
        for (edge, x) in &mut ordered {
            *x = edge.x(mid);
        }
        ordered.sort_unstable_by(|(a, x), (b, y)| {
            x.total_cmp(y)
                .then_with(|| a.slope().total_cmp(&b.slope()))
                .then(a.id.cmp(&b.id))
        });
        let mut winding = initial.clone();
        for &(edge, _) in &ordered {
            row.edge(edge, lo, hi, winding.step(edge)?)?;
        }
    }
    Ok(())
}

fn edges(
    shapes: &[Shape],
    size: (u32, u32),
    origin: (u32, u32),
    limits: &Limits,
    row: &mut Row,
) -> Result<Vec<Edge>> {
    let mut edges = Vec::new();
    let left = f64::from(origin.0);
    let right = left + f64::from(size.0);
    let top = f64::from(origin.1);
    let bottom = top + f64::from(size.1);
    for (group, shape) in shapes.iter().enumerate() {
        let contours = flatten(&shape.path, shape.transform, limits)?;
        let count = contours.len();
        for (i, contour) in contours.into_iter().enumerate() {
            if i + 1 == count && !contour.closed && contour.points.len() == 1 {
                continue;
            }
            let mut transformed: Vec<_> = contour
                .points
                .into_iter()
                .map(|p| p * shape.transform)
                .collect();
            if let Some(&point) = transformed.first()
                && contour.degenerate
            {
                // 8.5.3.3.1: a degenerate filled subpath encloses its device pixel.
                // Device choice at integral boundaries: the half-open floor cell.
                let (x, y) = (point.x.floor(), point.y.floor());
                transformed = vec![
                    Point::new(x, y),
                    Point::new(x + 1.0, y),
                    Point::new(x + 1.0, y + 1.0),
                    Point::new(x, y + 1.0),
                ];
            }
            let mut points = transformed.into_iter();
            let Some(first) = points.next() else { continue };
            let mut previous = first;
            for next in points.chain(std::iter::once(first)) {
                bounded(next)?;
                row.charge(3)?;
                clipped(
                    &mut edges,
                    previous,
                    next,
                    group,
                    shape.rule,
                    [left, top, right, bottom],
                    limits,
                )?;
                previous = next;
            }
        }
    }
    normalize_boundaries(edges, left, right, limits, row)
}

fn normalize_boundaries(
    edges: Vec<Edge>,
    left: f64,
    right: f64,
    limits: &Limits,
    row: &mut Row,
) -> Result<Vec<Edge>> {
    let mut output = Vec::new();
    let mut events = Vec::new();
    for edge in edges {
        if edge.a.x == edge.b.x && (edge.a.x == left || edge.a.x == right) {
            events.push((edge.group, edge.a.x, edge.a.y, edge.winding, edge.rule));
            events.push((edge.group, edge.a.x, edge.b.y, -edge.winding, edge.rule));
        } else {
            output.push(edge);
        }
    }
    row.sorted(events.len())?;
    events.sort_unstable_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.total_cmp(&b.1))
            .then(a.2.total_cmp(&b.2))
    });
    let mut previous = None;
    let mut winding = 0_i64;
    for (group, x, y, delta, rule) in events {
        row.charge(1)?;
        if let Some((previous_group, previous_x, previous_y)) = previous {
            if previous_group == group && previous_x == x {
                if y > previous_y && winding != 0 {
                    if output.len() >= limits.path_segments {
                        return Err(Error::limit("boundary edge budget exceeded"));
                    }
                    output.push(Edge {
                        a: Point::new(x, previous_y),
                        b: Point::new(x, y),
                        winding,
                        group,
                        rule,
                        id: 0,
                    });
                }
            } else {
                winding = 0;
            }
        }
        winding = winding
            .checked_add(delta)
            .ok_or(Error::limit("boundary winding overflow"))?;
        previous = Some((group, x, y));
    }
    for (id, edge) in output.iter_mut().enumerate() {
        edge.id = id;
    }
    Ok(output)
}

fn clipped(
    out: &mut Vec<Edge>,
    a: Point,
    b: Point,
    group: usize,
    rule: FillRule,
    bounds: [f64; 4],
    limits: &Limits,
) -> Result<()> {
    if a.y == b.y {
        return Ok(());
    }
    let (a, b, winding) = if a.y < b.y { (a, b, 1) } else { (b, a, -1) };
    let edge = Edge {
        a,
        b,
        winding,
        group,
        rule,
        id: 0,
    };
    let lo = a.y.max(bounds[1]);
    let hi = b.y.min(bounds[3]);
    if lo >= hi {
        return Ok(());
    }
    // At most two horizontal viewport crossings, plus the two endpoints.
    let mut events = [lo, hi, 0.0, 0.0];
    let mut count = 2;
    if a.x != b.x {
        for x in [bounds[0], bounds[2]] {
            let y = a.y + (b.y - a.y) * ((x - a.x) / (b.x - a.x));
            if y > lo && y < hi {
                events[count] = y;
                count += 1;
            }
        }
    }
    let events = &mut events[..count];
    events.sort_unstable_by(f64::total_cmp);
    for interval in events.windows(2) {
        if out.len() >= limits.path_segments {
            return Err(Error::limit("clipped edge budget exceeded"));
        }
        out.push(Edge {
            a: Point::new(edge.x(interval[0]).clamp(bounds[0], bounds[2]), interval[0]),
            b: Point::new(edge.x(interval[1]).clamp(bounds[0], bounds[2]), interval[1]),
            id: out.len(),
            ..edge
        });
    }
    Ok(())
}

fn inside(winding: i64, rule: FillRule) -> bool {
    match rule {
        FillRule::NonZero => winding != 0,
        FillRule::EvenOdd => winding % 2 != 0,
    }
}
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
    let start = ((-a) / (b - a)).clamp(0.0, 1.0);
    let end = ((1.0 - a) / (b - a)).clamp(0.0, 1.0);
    (end - start) * (a.max(0.0) + b.min(1.0)) * 0.5 + 1.0 - end
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to the allocated surface width before conversion"
)]
fn index(value: f64, max: usize) -> usize {
    value.clamp(0.0, max as f64) as usize
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "coverage is clamped and rounded to 0..=255"
)]
// A 1e-9 quantization-unit guard makes analytic half ties stable against
// cancellation in translated geometry (less than 4e-12 pixel area). The
// independent decimal-transform stroke oracle pins the half-up convention.
fn quantize(area: f64) -> u8 {
    (area.clamp(0.0, 1.0) * 255.0 + 0.5 + 1e-9)
        .floor()
        .min(255.0) as u8
}
pub(crate) fn charge(work: &mut usize, amount: usize, maximum: usize) -> Result<()> {
    *work = work
        .checked_add(amount)
        .filter(|&n| n <= maximum)
        .ok_or(Error::limit("raster work budget exceeded"))?;
    Ok(())
}
