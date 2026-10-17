//! Scanline sweep with signed cell areas and cover differences.
//!
//! Vertices are sorted once; only active edges visit a row. Non-crossing edges
//! contribute signed partial-cell area and a prefix cover difference, then one
//! scan over the row resolves coverage. Endpoint-order inversions identify
//! crossing blocks: exact integration is restricted to those blocks, preserving
//! winding selection before area integration. No all-pairs active-edge scan.
//! Horizontal clipping projects outside geometry onto viewport boundaries,
//! retaining its winding without visiting off-surface cells.

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
    row.sorted(edges.len())?;
    edges.sort_unstable_by(|a, b| a.a.y.total_cmp(&b.a.y));
    let mut active: Vec<Edge> = Vec::new();
    let mut next = 0;
    let mut vertices = Vec::new();
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
        vertices.clear();
        vertices.extend([lo, hi]);
        for edge in &active {
            if edge.a.y > lo {
                vertices.push(edge.a.y);
            }
            if edge.b.y < hi {
                vertices.push(edge.b.y);
            }
        }
        row.sorted(vertices.len())?;
        vertices.sort_unstable_by(f64::total_cmp);
        vertices.dedup();
        for interval in vertices.windows(2) {
            strip(
                &active,
                interval[0],
                interval[1],
                shapes.len(),
                &mut row,
                limits,
            )?;
        }
        row.charge(stride)?;
        row.finish(output);
    }
    Ok(())
}

fn strip(
    active: &[Edge],
    lo: f64,
    hi: f64,
    count: usize,
    row: &mut Row,
    limits: &Limits,
) -> Result<()> {
    let mid = lo.midpoint(hi);
    let mut top: Vec<_> = active
        .iter()
        .copied()
        .filter(|e| e.a.y < mid && e.b.y > mid)
        .collect();
    row.sorted(top.len())?;
    top.sort_unstable_by(|a, b| endpoint(*a, *b, lo, false));
    if top
        .windows(2)
        .all(|pair| endpoint(pair[0], pair[1], hi, true).is_le())
    {
        let mut winding = Windings::new(count);
        for edge in top {
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
    let mut winding = Windings::new(count);
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
            crossing(block, lo, hi, &winding, row, limits)?;
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

// Only endpoint-order inversions can cross in a strip. Merge reporting costs
// O(n log n + crossings), rather than comparing every active pair.
fn crossings(
    sorted: &mut [Edge],
    lo: f64,
    hi: f64,
    events: &mut Vec<f64>,
    row: &mut Row,
    limits: &Limits,
) -> Result<()> {
    if sorted.len() < 2 {
        return Ok(());
    }
    let middle = sorted.len() / 2;
    let (left, right) = sorted.split_at_mut(middle);
    crossings(left, lo, hi, events, row, limits)?;
    crossings(right, lo, hi, events, row, limits)?;
    let mut merged = Vec::with_capacity(left.len() + right.len());
    let (mut i, mut j) = (0, 0);
    while i < left.len() && j < right.len() {
        row.charge(1)?;
        let a = *left
            .get(i)
            .ok_or(Error::invalid_argument("invalid merge index"))?;
        let b = *right
            .get(j)
            .ok_or(Error::invalid_argument("invalid merge index"))?;
        if endpoint(a, b, hi, true).is_le() {
            merged.push(a);
            i += 1;
        } else {
            for a in left.iter().skip(i) {
                row.charge(1)?;
                let d0 = a.x(lo) - b.x(lo);
                let d1 = a.x(hi) - b.x(hi);
                if (d0 < 0.0 && d1 > 0.0) || (d0 > 0.0 && d1 < 0.0) {
                    if events.len() >= limits.path_segments {
                        return Err(Error::limit("crossing event budget exceeded"));
                    }
                    events.push(lo + (hi - lo) * (d0 / (d0 - d1)));
                }
            }
            merged.push(b);
            j += 1;
        }
    }
    merged.extend(left.iter().skip(i).copied());
    merged.extend(right.iter().skip(j).copied());
    sorted.copy_from_slice(&merged);
    Ok(())
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
    let mut sorted = block.to_vec();
    crossings(&mut sorted, lo, hi, &mut events, row, limits)?;
    row.sorted(events.len())?;
    events.sort_unstable_by(f64::total_cmp);
    events.dedup();
    for interval in events.windows(2) {
        let (lo, hi) = (interval[0], interval[1]);
        row.sorted(sorted.len())?;
        sorted.sort_unstable_by(|a, b| endpoint(*a, *b, lo.midpoint(hi), false));
        let mut winding = initial.clone();
        for &edge in &sorted {
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
        for contour in flatten(&shape.path, shape.transform, limits)? {
            let mut points = contour.points.into_iter().map(|p| p * shape.transform);
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
    let mut events = vec![lo, hi];
    if a.x != b.x {
        for x in [bounds[0], bounds[2]] {
            let y = a.y + (b.y - a.y) * ((x - a.x) / (b.x - a.x));
            if y > lo && y < hi {
                events.push(y);
            }
        }
    }
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
