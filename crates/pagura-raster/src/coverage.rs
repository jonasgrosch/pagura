//! Exact-area scanlines with signed cell areas and cover differences.
//!
//! One active-edge order follows prepared vertex events across scanlines.
//! Endpoint inversions partition crossing blocks and discover their events in
//! one pass. Winding boundaries are selected before integration; partial-cell
//! areas and prefix cover differences resolve at touched cells and constant runs.
//! Horizontal clipping projects outside geometry onto viewport boundaries,
//! preserving winding and bounding work by the surface.

use crate::flatten::{CurveStack, Polyline, bounded, flatten_into};
use crate::{FillRule, IntRect, Path, Point, Transform, quantize_unit};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Limits, Meter, Result};
use std::sync::Arc;

mod accumulation;
mod admission;
mod rectangle;
use rectangle::Rectangle;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Geometry {
    Path(Arc<Path>),
    Glyph(Arc<crate::GlyphGeometry>),
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shape {
    pub(crate) geometry: Geometry,
    pub(crate) transform: Transform,
    pub(crate) rule: FillRule,
}
impl Shape {
    fn segments(&self) -> usize {
        match &self.geometry {
            Geometry::Path(path) => path.segments().len(),
            Geometry::Glyph(glyph) => glyph.vertices(),
        }
    }
    /// Whether this shape is a rectangle containing every pixel of `area`.
    pub(crate) fn covers(&self, area: IntRect) -> bool {
        self.rectangle().is_some_and(|rectangle| {
            rectangle.left <= f64::from(area.x())
                && rectangle.top <= f64::from(area.y())
                && rectangle.right >= f64::from(area.right())
                && rectangle.bottom >= f64::from(area.bottom())
        })
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

#[derive(Debug, Default)]
struct Windings {
    values: Vec<i64>,
    outside: usize,
}
impl Windings {
    fn reset(&mut self) {
        self.values.fill(0);
        self.outside = self.values.len();
    }
    // Closing a span contributes +area; opening contributes -area.
    fn step(&mut self, edge: Edge) -> Result<i32> {
        let was_inside = self.outside == 0;
        // Groups number the non-rectangle shapes, and coverage() sizes the
        // winding values to their count before building any edge.
        let value = &mut self.values[edge.group];
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

/// An active edge in bottom order, with its top-order index and its x at
/// the current evaluation height.
#[derive(Debug, Clone, Copy)]
struct Ordered {
    owner: usize,
    edge: Edge,
    x: f64,
}

// Scratch belongs to one coverage sweep and retains capacity between strips.
#[derive(Debug, Default)]
struct Sweep {
    // Certified at the previous strip end; admissions invalidate this order.
    sorted: bool,
    events: Vec<(usize, f64)>,
    ordered: Vec<Ordered>,
    winding: Windings,
}

/// The message of every raster work budget error.
pub(crate) const RASTER_WORK: &str = "raster work budget exceeded";

#[derive(Debug)]
struct Row {
    areas: Vec<f64>,
    covers: Vec<f64>,
    touched: Vec<u64>,
    origin: f64,
    work: Meter,
    maximum_events: usize,
}
impl Default for Row {
    fn default() -> Self {
        Self {
            areas: Vec::new(),
            covers: Vec::new(),
            touched: Vec::new(),
            origin: 0.0,
            work: Meter::new(0, RASTER_WORK),
            maximum_events: 0,
        }
    }
}
impl Row {
    fn reset(&mut self, width: usize, origin: u32) -> Result<()> {
        // A prior budget error may have left a partially integrated row.
        for x in take_cells(&mut self.touched) {
            self.areas[x] = 0.0;
            self.covers[x] = 0.0;
        }
        let words = width.div_ceil(64);
        self.touched
            .try_reserve_exact(words.saturating_sub(self.touched.len()))
            .map_err(|_| Error::limit("scanline bitset allocation failed"))?;
        self.touched.resize(words, 0);
        for buffer in [&mut self.areas, &mut self.covers] {
            buffer
                .try_reserve_exact(width.saturating_sub(buffer.len()))
                .map_err(|_| Error::limit("scanline allocation failed"))?;
            buffer.resize(width, 0.0);
        }
        self.origin = f64::from(origin);
        Ok(())
    }
    fn charge(&mut self, amount: usize) -> Result<()> {
        self.work.charge(amount)
    }
    fn sorted(&mut self, count: usize) -> Result<()> {
        self.charge(sort_work(count)?)
    }
    fn edge(&mut self, edge: Edge, lo: f64, hi: f64, sign: i32) -> Result<()> {
        if sign == 0 {
            return Ok(());
        }
        let a = edge.x(lo) - self.origin;
        let b = edge.x(hi) - self.origin;
        let start = index(a.min(b).floor(), self.areas.len());
        let end = index(a.max(b).ceil(), self.areas.len());
        let area = (hi - lo) * f64::from(sign);
        self.charge(end.saturating_sub(start) + 1)?;
        if start > 0 {
            if let Some(first) = self.covers.first_mut() {
                *first += area;
                self.touched[0] |= 1;
            }
            if let Some(after) = self.covers.get_mut(start) {
                *after -= area;
                self.touched[start / 64] |= 1 << (start % 64);
            }
        }
        // index() clamps both ends to the row and start <= end, so the slice
        // is in bounds even for geometry beyond the viewport.
        for (x, target) in (start..end).zip(&mut self.areas[start..end]) {
            let column =
                f64::from(u32::try_from(x).map_err(|_| Error::limit("column exceeds u32"))?);
            *target += area * ramp(a - column, b - column);
            self.touched[x / 64] |= 1 << (x % 64);
        }
        Ok(())
    }
    fn finish(&mut self, output: &mut [u8]) -> Result<()> {
        let cells: usize = self.touched.iter().map(|w| w.count_ones() as usize).sum();
        // Charge both bitset passes as well as cells, before draining any state.
        self.charge(cells + 2 * self.touched.len() + 1)?;
        let mut cover = 0.0;
        let mut left = 0;
        // Untouched entries are +0.0. Visiting written deltas in increasing x
        // preserves the dense scan's addition order; omitted +0.0 additions
        // cannot change coverage. Area is zero between touched cells, so every
        // pixel in a gap has quantize_unit(cover). Output was cleared once per mask.
        for x in take_cells(&mut self.touched) {
            fill_run(&mut output[left..x], cover);
            cover += std::mem::take(&mut self.covers[x]);
            output[x] = quantize_unit(std::mem::take(&mut self.areas[x]) + cover);
            left = x + 1;
        }
        fill_run(&mut output[left..], cover);
        Ok(())
    }
}

// Draining clears only nonzero bitset words and yields each written cell once.
fn take_cells(words: &mut [u64]) -> impl Iterator<Item = usize> + '_ {
    words.iter_mut().enumerate().flat_map(|(word, marked)| {
        let mut bits = *marked;
        if bits != 0 {
            *marked = 0;
        }
        std::iter::from_fn(move || {
            if bits == 0 {
                return None;
            }
            let x = word * 64 + bits.trailing_zeros() as usize;
            bits &= bits - 1;
            Some(x)
        })
    })
}
fn fill_run(output: &mut [u8], cover: f64) {
    if !output.is_empty() {
        let value = quantize_unit(cover);
        if value != 0 {
            output.fill(value);
        }
    }
}

/// One end of a vertical edge on the viewport's left or right side.
#[derive(Debug, Clone, Copy)]
struct BoundaryEvent {
    group: usize,
    x: f64,
    y: f64,
    delta: i64,
    rule: FillRule,
}

/// Temporary sweep storage; no path or coverage is cached.
#[derive(Debug, Default)]
pub(crate) struct Exact {
    row: Row,
    contours: Vec<Polyline>,
    curves: CurveStack,
    edges: Vec<Edge>,
    raw: Vec<Edge>,
    boundary: Vec<BoundaryEvent>,
    vertices: Vec<f64>,
    active: Vec<Edge>,
    winding: Windings,
    sweep: Sweep,
}
impl Exact {
    pub(crate) fn retained_bytes(&self) -> usize {
        let buffers = [
            capacity_bytes(&self.row.areas),
            capacity_bytes(&self.row.covers),
            capacity_bytes(&self.row.touched),
            capacity_bytes(&self.edges),
            capacity_bytes(&self.raw),
            capacity_bytes(&self.boundary),
            capacity_bytes(&self.vertices),
            capacity_bytes(&self.active),
            capacity_bytes(&self.winding.values),
            capacity_bytes(&self.sweep.winding.values),
            capacity_bytes(&self.sweep.events),
            capacity_bytes(&self.sweep.ordered),
            capacity_bytes(&self.contours),
            capacity_bytes(&self.curves),
        ];
        buffers
            .into_iter()
            .chain(
                self.contours
                    .iter()
                    .map(|contour| capacity_bytes(&contour.points)),
            )
            .fold(0, usize::saturating_add)
    }
}

/// The bytes a vector's capacity holds, saturating.
pub(crate) fn capacity_bytes<T>(values: &Vec<T>) -> usize {
    values.capacity().saturating_mul(size_of::<T>())
}

/// Reused sweep storage; the larger integer workspace is initialized on demand.
#[derive(Debug, Default)]
pub(crate) struct Scratch {
    exact: Exact,
    accumulation: Vec<accumulation::Accumulator>,
    exact_bytes: usize,
    accumulation_bytes: usize,
}

impl Scratch {
    fn accumulator(&mut self) -> Result<&mut accumulation::Accumulator> {
        if self.accumulation.is_empty() {
            self.accumulation
                .try_reserve_exact(1)
                .map_err(|_| Error::limit("raster accumulator allocation failed"))?;
            self.accumulation.push(accumulation::Accumulator::default());
        }
        Ok(&mut self.accumulation[0])
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.exact_bytes.saturating_add(self.accumulation_bytes)
    }
}

/// The rectangle intersection of a shape set and its count of other shapes,
/// computed before either backend runs.
#[derive(Debug, Clone, Copy)]
struct Prepared {
    bounds: Rectangle,
    groups: usize,
}

/// Integrates the intersection of `shapes` over `area` into `out`, one byte
/// per pixel, row-major.
pub(crate) fn coverage(
    area: IntRect,
    shapes: &[Shape],
    out: &mut [u8],
    limits: &Limits,
    scratch: &mut Scratch,
) -> Result<()> {
    let prepared = scratch.exact.prepare(shapes, area, limits)?;
    if prepared.groups == 0 && !shapes.is_empty() {
        scratch.exact.row.charge(out.len())?;
        return prepared.bounds.write(out, u32_to_usize(area.width()), area);
    }
    if let Some(anchor) = admission::sweep_anchor(shapes, limits.path_segments) {
        let result = scratch
            .exact
            .render(area, shapes, out, limits, prepared, anchor);
        scratch.exact_bytes = scratch.exact.retained_bytes();
        result
    } else {
        let work = scratch.exact.row.work.used();
        let accumulator = scratch.accumulator()?;
        let origin = (area.x(), area.y());
        let size = (area.width(), area.height());
        let result = accumulator.render(shapes, out, origin, size, limits, work);
        let retained = accumulator.retained_bytes();
        scratch.accumulation_bytes = capacity_bytes(&scratch.accumulation).saturating_add(retained);
        result
    }
}

#[cfg(test)]
pub(crate) fn coverage_accumulated(
    area: IntRect,
    shapes: &[Shape],
    out: &mut [u8],
    limits: &Limits,
    scratch: &mut Scratch,
) -> Result<()> {
    // Exercise accumulation even for rectangles and short component contours.
    let origin = (area.x(), area.y());
    let size = (area.width(), area.height());
    scratch
        .accumulator()?
        .render(shapes, out, origin, size, limits, 0)
}

#[cfg(test)]
fn coverage_exact(
    area: IntRect,
    shapes: &[Shape],
    out: &mut [u8],
    limits: &Limits,
    scratch: &mut Exact,
) -> Result<()> {
    let prepared = scratch.prepare(shapes, area, limits)?;
    scratch.render(area, shapes, out, limits, prepared, Point::default())
}

impl Exact {
    fn prepare(&mut self, shapes: &[Shape], area: IntRect, limits: &Limits) -> Result<Prepared> {
        self.row.work = Meter::new(limits.raster_work, RASTER_WORK);
        self.row.maximum_events = limits.path_segments;
        let (bounds, groups) = Rectangle::intersection(shapes, area, &mut self.row)?;
        Ok(Prepared { bounds, groups })
    }

    /// Sweeps `shapes` with edges translated by `anchor`, an integer point
    /// near the geometry that keeps strip coordinates small.
    fn render(
        &mut self,
        area: IntRect,
        shapes: &[Shape],
        out: &mut [u8],
        limits: &Limits,
        Prepared { bounds, groups }: Prepared,
        anchor: Point,
    ) -> Result<()> {
        let stride = u32_to_usize(area.width());
        if groups == 0 && !shapes.is_empty() {
            self.row.charge(out.len())?;
            return bounds.write(out, stride, area);
        }
        let bounds = Rectangle {
            left: bounds.left - anchor.x,
            right: bounds.right - anchor.x,
            top: bounds.top - anchor.y,
            bottom: bounds.bottom - anchor.y,
        };
        self.row.reset(stride, area.x())?;
        self.row.origin -= anchor.x;
        self.edges.clear();
        self.raw.clear();
        self.boundary.clear();
        build_edges(
            shapes,
            (bounds, anchor),
            limits,
            &mut self.row,
            &mut self.raw,
            &mut self.contours,
            &mut self.curves,
        )?;
        normalize_boundaries(
            &mut self.raw,
            &mut self.edges,
            &mut self.boundary,
            bounds.left,
            bounds.right,
            limits,
            &mut self.row,
        )?;
        self.sweep_edges(stride, out, area.y(), groups, anchor.y)
    }

    fn sweep_edges(
        &mut self,
        stride: usize,
        out: &mut [u8],
        top: u32,
        groups: usize,
        anchor_y: f64,
    ) -> Result<()> {
        let Self {
            row,
            edges,
            vertices,
            active,
            winding,
            sweep,
            ..
        } = self;
        // A shared vertical extent has just two vertex events, already ordered.
        // Keep them inline instead of allocating and sorting duplicate endpoints.
        row.charge(edges.len())?;
        let extent = edges
            .first()
            .map(|e| [e.a.y, e.b.y])
            .filter(|&[lo, hi]| edges.iter().all(|e| e.a.y == lo && e.b.y == hi));
        vertices.clear();
        if extent.is_none() {
            row.sorted(edges.len())?;
            edges.sort_unstable_by(|a, b| a.a.y.total_cmp(&b.a.y));
            vertices.extend(edges.iter().flat_map(|e| [e.a.y, e.b.y]));
            row.sorted(vertices.len())?;
            vertices.sort_unstable_by(f64::total_cmp);
            vertices.dedup();
        }
        let mut vertices = extent
            .into_iter()
            .flatten()
            .chain(vertices.iter().copied())
            .peekable();
        active.clear();
        let mut admitted = 0;
        winding.values.resize(groups, 0);
        sweep.sorted = false;
        out.fill(0);
        for (y, output) in out.chunks_exact_mut(stride).enumerate() {
            let lo = f64::from(u32::try_from(y).map_err(|_| Error::limit("row exceeds u32"))?)
                + f64::from(top)
                - anchor_y;
            let hi = lo + 1.0;
            while vertices.next_if(|&y| y <= lo).is_some() {}
            if active.is_empty() && edges.get(admitted).is_none_or(|e| e.a.y >= hi) {
                continue;
            }
            let mut start = lo;
            loop {
                let end = vertices.peek().copied().unwrap_or(hi).min(hi);
                let mid = start.midpoint(end);
                row.charge(active.len())?;
                active.retain(|e| e.b.y > mid);
                // One starting-y stream admits each edge once per mask. Surviving
                // edges retain the previous interval's order, including across rows.
                while let Some(&edge) = edges.get(admitted)
                    && edge.a.y < mid
                {
                    row.charge(1)?;
                    if edge.b.y > mid {
                        sweep.sorted = false;
                        active.push(edge);
                    }
                    admitted += 1;
                }
                if !active.is_empty() {
                    strip(active, start, end, winding, row, sweep)?;
                }
                if end == hi {
                    break;
                }
                vertices.next();
                start = end;
            }
            row.finish(output)?;
        }
        Ok(())
    }
}

fn strip(
    top: &mut [Edge],
    lo: f64,
    hi: f64,
    winding: &mut Windings,
    row: &mut Row,
    sweep: &mut Sweep,
) -> Result<()> {
    winding.reset();
    if !sweep.sorted {
        repair(top, |a, b| cached(*a, a.x(lo), *b, b.x(lo), false), row)?;
    }
    sweep.sorted = true;
    // Equal x may meet at hi without crossing inside this strip. The complete
    // endpoint key also certifies the next strip's top order after departures.
    let mut previous = None;
    if top.iter().all(|&b| {
        let bx = b.x(hi);
        let ordered = previous.is_none_or(|(a, ax)| {
            sweep.sorted &= cached(a, ax, b, bx, false).is_le();
            ax.total_cmp(&bx).is_le()
        });
        previous = Some((b, bx));
        ordered
    }) {
        for &edge in top.iter() {
            row.edge(edge, lo, hi, winding.step(edge)?)?;
        }
        return Ok(());
    }
    sweep.events.clear();
    sweep.ordered.clear();
    sweep
        .ordered
        .extend(top.iter().enumerate().map(|(owner, &edge)| Ordered {
            owner,
            edge,
            x: edge.x(hi),
        }));
    row.charge(top.len())?;
    // One bottom-order insertion pass discovers every inversion once. The
    // inserted edge keeps its original top index throughout its exchanges;
    // event owners are therefore monotone, even before sorting by event y.
    for i in 1..sweep.ordered.len() {
        let mut j = i;
        while j > 0 {
            row.charge(1)?;
            let Ordered { edge: a, x: ax, .. } = sweep.ordered[j - 1];
            let Ordered {
                owner,
                edge: b,
                x: bx,
            } = sweep.ordered[j];
            if cached(a, ax, b, bx, true).is_le() {
                break;
            }
            let (d0, d1) = (a.x(lo) - b.x(lo), ax - bx);
            if d0 < 0.0 && d1 > 0.0 {
                if sweep.events.len() >= row.maximum_events {
                    return Err(Error::limit("crossing event budget exceeded"));
                }
                sweep
                    .events
                    .push((owner, lo + (hi - lo) * (d0 / (d0 - d1))));
            }
            sweep.ordered.swap(j - 1, j);
            j -= 1;
        }
    }
    let mut events = sweep.events.as_mut_slice();
    let (mut start, mut end) = (0, 0);
    for i in 0..sweep.ordered.len() {
        end = end.max(sweep.ordered[i].owner);
        if end != i {
            continue;
        }
        let block = &top[start..=i];
        if let [edge] = block {
            row.edge(*edge, lo, hi, winding.step(*edge)?)?;
        } else {
            // The maximum original index closes precisely an irreducible
            // permutation block. Its events form the next contiguous prefix.
            let count = events.partition_point(|&(owner, _)| owner <= i);
            let (current, rest) = events.split_at_mut(count);
            events = rest;
            let ordered = &mut sweep.ordered[start..=i];
            for (entry, &original) in ordered.iter_mut().zip(block) {
                entry.edge = original;
            }
            crossing(ordered, current, lo, hi, winding, row, &mut sweep.winding)?;
            for &edge in block {
                winding.step(edge)?;
            }
        }
        start = i + 1;
    }
    // Each block now retains its last interval's order for the next strip.
    for (edge, ordered) in top.iter_mut().zip(&sweep.ordered) {
        *edge = ordered.edge;
    }
    Ok(())
}

// Repair sparse inversions in place. After one slice-length of exchanges,
// sorting bounds a badly disturbed admission order by O(n log n).
fn repair<T>(
    values: &mut [T],
    mut compare: impl FnMut(&T, &T) -> std::cmp::Ordering,
    row: &mut Row,
) -> Result<()> {
    row.charge(values.len())?;
    let mut remaining = values.len();
    for i in 1..values.len() {
        let mut j = i;
        // i and j are internal loop indices, always within this slice. Keep
        // the key at i until its insertion point is known, then move once.
        while j > 0 && compare(&values[j - 1], &values[i]).is_gt() {
            if remaining == 0 {
                row.sorted(values.len())?;
                values.sort_unstable_by(compare);
                return Ok(());
            }
            row.charge(1)?;
            remaining -= 1;
            j -= 1;
        }
        values[j..=i].rotate_right(1);
    }
    Ok(())
}

fn cached(a: Edge, ax: f64, b: Edge, bx: f64, bottom: bool) -> std::cmp::Ordering {
    ax.total_cmp(&bx)
        .then_with(|| {
            let (a, b) = if bottom { (b, a) } else { (a, b) };
            a.slope().total_cmp(&b.slope())
        })
        .then(a.id.cmp(&b.id))
}

fn crossing(
    ordered: &mut [Ordered],
    events: &mut [(usize, f64)],
    lo: f64,
    hi: f64,
    initial: &Windings,
    row: &mut Row,
    winding: &mut Windings,
) -> Result<()> {
    row.sorted(events.len().saturating_add(2))?;
    events.sort_unstable_by(|(_, a), (_, b)| a.total_cmp(b));
    let mut start = lo;
    for end in events.iter().map(|&(_, y)| y).chain(std::iter::once(hi)) {
        if start == end {
            continue;
        }
        let mid = start.midpoint(end);
        for entry in ordered.iter_mut() {
            entry.x = entry.edge.x(mid);
        }
        repair(ordered, |a, b| cached(a.edge, a.x, b.edge, b.x, false), row)?;
        winding.values.clone_from(&initial.values);
        winding.outside = initial.outside;
        for entry in ordered.iter() {
            row.edge(entry.edge, start, end, winding.step(entry.edge)?)?;
        }
        start = end;
    }
    Ok(())
}

fn build_edges(
    shapes: &[Shape],
    (bounds, anchor): (Rectangle, Point),
    limits: &Limits,
    row: &mut Row,
    edges: &mut Vec<Edge>,
    contours: &mut Vec<Polyline>,
    curves: &mut CurveStack,
) -> Result<()> {
    for (group, shape) in shapes
        .iter()
        .filter(|shape| shape.rectangle().is_none())
        .enumerate()
    {
        match &shape.geometry {
            Geometry::Glyph(glyph) => {
                if glyph.vertices() > limits.path_segments {
                    return Err(Error::limit("flattened segment budget exceeded"));
                }
                // The same placement pays for its geometry on cache hits and misses.
                row.charge(glyph.vertices())?;
                for (i, contour) in glyph.contours().iter().enumerate() {
                    if i + 1 == glyph.contours().len()
                        && !contour.is_closed()
                        && contour.points().len() == 1
                    {
                        continue;
                    }
                    emit_contour(
                        contour.points().iter().map(|&p| p * shape.transform),
                        contour.is_degenerate(),
                        (group, shape.rule),
                        (bounds, anchor),
                        limits,
                        row,
                        edges,
                    )?;
                }
            }
            Geometry::Path(path) => {
                let count = flatten_into(path, shape.transform, limits, contours, curves)?;
                contours.truncate(count);
                for (i, contour) in contours.iter().enumerate() {
                    if i + 1 == count && !contour.closed && contour.points.len() == 1 {
                        continue;
                    }
                    emit_contour(
                        contour.points.iter().map(|&p| p * shape.transform),
                        contour.degenerate,
                        (group, shape.rule),
                        (bounds, anchor),
                        limits,
                        row,
                        edges,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn emit_contour(
    mut points: impl Iterator<Item = Point>,
    degenerate: bool,
    (group, rule): (usize, FillRule),
    (bounds, anchor): (Rectangle, Point),
    limits: &Limits,
    row: &mut Row,
    edges: &mut Vec<Edge>,
) -> Result<()> {
    let Some(first) = points.next() else {
        return Ok(());
    };
    let corners = if degenerate {
        let (x, y) = (first.x.floor(), first.y.floor());
        Some([
            Point::new(x, y),
            Point::new(x + 1.0, y),
            Point::new(x + 1.0, y + 1.0),
            Point::new(x, y + 1.0),
        ])
    } else {
        None
    };
    let mut emit = |points: &mut dyn Iterator<Item = Point>| -> Result<()> {
        let Some(first) = points.next() else {
            return Ok(());
        };
        let mut previous = Point::new(first.x - anchor.x, first.y - anchor.y);
        for next in points.chain(std::iter::once(first)) {
            bounded(next)?;
            let next = Point::new(next.x - anchor.x, next.y - anchor.y);
            row.charge(3)?;
            if !bounds.is_empty() {
                clipped(edges, previous, next, group, rule, bounds, limits)?;
            }
            previous = next;
        }
        Ok(())
    };
    if let Some(corners) = corners {
        emit(&mut corners.into_iter())
    } else {
        emit(&mut std::iter::once(first).chain(points))
    }
}

fn normalize_boundaries(
    edges: &mut Vec<Edge>,
    output: &mut Vec<Edge>,
    events: &mut Vec<BoundaryEvent>,
    left: f64,
    right: f64,
    limits: &Limits,
    row: &mut Row,
) -> Result<()> {
    for edge in edges.drain(..) {
        if edge.a.x == edge.b.x && (edge.a.x == left || edge.a.x == right) {
            let event = |y, delta| BoundaryEvent {
                group: edge.group,
                x: edge.a.x,
                y,
                delta,
                rule: edge.rule,
            };
            events.extend([
                event(edge.a.y, edge.winding),
                event(edge.b.y, -edge.winding),
            ]);
        } else {
            output.push(edge);
        }
    }
    row.sorted(events.len())?;
    events.sort_unstable_by(|a, b| {
        a.group
            .cmp(&b.group)
            .then(a.x.total_cmp(&b.x))
            .then(a.y.total_cmp(&b.y))
    });
    let mut previous = None;
    let mut winding = 0_i64;
    for BoundaryEvent {
        group,
        x,
        y,
        delta,
        rule,
    } in events.drain(..)
    {
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
    Ok(())
}

fn clipped(
    out: &mut Vec<Edge>,
    a: Point,
    b: Point,
    group: usize,
    rule: FillRule,
    bounds: Rectangle,
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
    let lo = a.y.max(bounds.top);
    let hi = b.y.min(bounds.bottom);
    if lo >= hi {
        return Ok(());
    }
    // At most two horizontal viewport crossings, plus the two endpoints.
    let mut events = [lo, hi, 0.0, 0.0];
    let mut count = 2;
    if a.x != b.x {
        for x in [bounds.left, bounds.right] {
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
            a: Point::new(
                edge.x(interval[0]).clamp(bounds.left, bounds.right),
                interval[0],
            ),
            b: Point::new(
                edge.x(interval[1]).clamp(bounds.left, bounds.right),
                interval[1],
            ),
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
fn sort_work(count: usize) -> Result<usize> {
    count
        .checked_mul(count.max(1).ilog2() as usize + 1)
        .ok_or(Error::limit("sort work overflow"))
}

/// The integer-cell backend's own work counter, until it charges a [`Meter`]
/// like the sweep.
pub(crate) fn charge(work: &mut usize, amount: usize, maximum: usize) -> Result<()> {
    *work = work
        .checked_add(amount)
        .filter(|&n| n <= maximum)
        .ok_or(Error::limit(RASTER_WORK))?;
    Ok(())
}

#[cfg(test)]
mod tests;
