//! Exact-area scanlines with signed cell areas and cover differences.
//!
//! One active-edge order follows prepared vertex events across scanlines.
//! Endpoint inversions partition crossing blocks and discover their events in
//! one pass. Winding boundaries are selected before integration; partial-cell
//! areas and prefix cover differences resolve at touched cells and constant runs.
//! Horizontal clipping projects outside geometry onto viewport boundaries,
//! preserving winding and bounding work by the surface.

use crate::flatten::{CurveStack, Polyline, bounded, flatten_into};
use crate::{FillRule, Path, Point, Transform, quantize_unit};
use pagura_core::{Error, Limits, Meter, Result};
use std::sync::Arc;

mod accumulation;
mod admission;
mod approximate;
pub(crate) mod quadratic;
mod rectangle;
mod signed;
pub(crate) use rectangle::Rectangle;
pub(crate) use rectangle::Rows;

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
    pub(crate) fn segments(&self) -> usize {
        match &self.geometry {
            Geometry::Path(path) => path.segments().len(),
            Geometry::Glyph(glyph) => glyph.vertices(),
        }
    }
    pub(crate) fn covers(&self, origin: (u32, u32), size: (u32, u32)) -> bool {
        self.rectangle().is_some_and(|rectangle| {
            rectangle.left <= f64::from(origin.0)
                && rectangle.top <= f64::from(origin.1)
                && rectangle.right >= f64::from(origin.0) + f64::from(size.0)
                && rectangle.bottom >= f64::from(origin.1) + f64::from(size.1)
        })
    }
}
// Successfully constructed masks contain finite geometry only.
impl Eq for Shape {}

#[derive(Debug, Clone, Copy, PartialEq)]
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

// Scratch belongs to one coverage sweep and retains capacity between strips.
#[derive(Debug, Default)]
struct Sweep {
    // Certified at the previous strip end; admissions invalidate this order.
    sorted: bool,
    events: Vec<(usize, f64)>,
    ordered: Vec<(usize, Edge, f64)>,
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

type BoundaryEvent = (usize, f64, f64, i64, FillRule);

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
            bytes(&self.row.areas),
            bytes(&self.row.covers),
            bytes(&self.row.touched),
            bytes(&self.edges),
            bytes(&self.raw),
            bytes(&self.boundary),
            bytes(&self.vertices),
            bytes(&self.active),
            bytes(&self.winding.values),
            bytes(&self.sweep.winding.values),
            bytes(&self.sweep.events),
            bytes(&self.sweep.ordered),
            bytes(&self.contours),
            bytes(&self.curves),
        ];
        buffers
            .into_iter()
            .chain(self.contours.iter().map(|contour| bytes(&contour.points)))
            .fold(0, usize::saturating_add)
    }
}

fn bytes<T>(values: &Vec<T>) -> usize {
    values.capacity().saturating_mul(size_of::<T>())
}

/// Reused sweep storage; the larger integer workspace is initialized on demand.
#[derive(Debug, Default)]
pub(crate) struct Scratch {
    exact: Exact,
    pub(crate) approximate: Option<f64>,
    pub(crate) geometric_compounds: bool,
    cells: approximate::Cells,
    accumulation: Vec<accumulation::Accumulator>,
    curves: Vec<quadratic::Sweep>,
    signed: signed::Signed,
    exact_bytes: usize,
    accumulation_bytes: usize,
    curve_bytes: usize,
    signed_bytes: usize,
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

    fn curves(&mut self) -> Result<&mut quadratic::Sweep> {
        if self.curves.is_empty() {
            self.curves
                .try_reserve_exact(1)
                .map_err(|_| Error::limit("curve sweep allocation failed"))?;
            self.curves.push(quadratic::Sweep::default());
        }
        Ok(&mut self.curves[0])
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.exact_bytes
            .saturating_add(self.cells.retained_bytes())
            .saturating_add(self.accumulation_bytes)
            .saturating_add(self.curve_bytes)
            .saturating_add(self.signed_bytes)
    }
}

pub(crate) fn coverage(
    width: u32,
    height: u32,
    shapes: &[Shape],
    out: &mut [u8],
    origin: (u32, u32),
    limits: &Limits,
    scratch: &mut Scratch,
) -> Result<()> {
    let (adjusted, compound) = compound_admission(shapes, limits, scratch.geometric_compounds)?;
    let limits = adjusted.as_ref().unwrap_or(limits);
    if let Some(tolerance) = scratch.approximate
        && !compound
        && let [shape] = shapes
        && shape.rectangle().is_none()
    {
        if shape.segments() > limits.path_segments || limits.mask_sources == 0 {
            return Err(Error::limit("approximate source budget exceeded"));
        }
        // A retained boundary is already a geometric stroke-union proof. The
        // exact backend's component-frame admission only chooses its rounding
        // path; signed cells do not need to repeat that traversal.
        let boundary;
        let source = if let Geometry::Path(path) = &shape.geometry
            && let Some(path) = &path.boundary
            && shape.rule == FillRule::NonZero
        {
            boundary = Shape {
                geometry: Geometry::Path(Arc::clone(path)),
                ..shape.clone()
            };
            &boundary
        } else {
            shape
        };
        if scratch
            .cells
            .render(source, out, origin, (width, height), limits, tolerance)?
        {
            return Ok(());
        }
    }
    coverage_geometric(width, height, shapes, out, origin, limits, scratch)
}

fn compound_admission(
    shapes: &[Shape],
    limits: &Limits,
    enabled: bool,
) -> Result<(Option<Limits>, bool)> {
    if enabled
        && let [
            Shape {
                geometry: Geometry::Path(path),
                rule,
                ..
            },
        ] = shapes
        && path.boundary.is_none()
        && path.fill_region(*rule).is_none()
    {
        if path.segments().len() > limits.path_segments {
            return Err(Error::limit("path segment budget exceeded"));
        }
        let adjusted = Limits {
            raster_work: limits
                .raster_work
                .checked_sub(path.segments().len())
                .ok_or(Error::limit("compound admission work exceeded"))?,
            ..*limits
        };
        let compound = path
            .segments()
            .iter()
            .filter(|s| matches!(s, crate::Segment::MoveTo(_)))
            .take(2)
            .count()
            == 2;
        Ok((Some(adjusted), compound))
    } else {
        Ok((None, false))
    }
}

fn coverage_geometric(
    width: u32,
    height: u32,
    shapes: &[Shape],
    out: &mut [u8],
    origin: (u32, u32),
    limits: &Limits,
    scratch: &mut Scratch,
) -> Result<()> {
    let boundary = admission::stroke_boundary(shapes, limits.path_segments);
    let substituted;
    let (shapes, boundary_anchor) = match boundary {
        Some((boundary, anchor)) => {
            substituted = [std::slice::from_ref(&boundary), &shapes[1..]].concat();
            (substituted.as_slice(), Some(anchor))
        }
        None => (shapes, None),
    };
    // Admitted glyphs take the curve sweep. Everything else runs the line
    // sweep or accumulation below, which never see a curve.
    if let Some(anchor) = quadratic::anchor(shapes) {
        let result = scratch
            .curves()?
            .render((width, height), shapes, out, origin, limits, anchor);
        scratch.curve_bytes =
            bytes(&scratch.curves).saturating_add(scratch.curves[0].retained_bytes());
        return result;
    }
    let prepared = scratch
        .exact
        .prepare(shapes, origin, (width, height), limits)?;
    if prepared.1 == 0 && !shapes.is_empty() {
        scratch.exact.row.charge(out.len())?;
        return prepared.0.write(out, width as usize, origin);
    }
    // Certified glyphs accumulate signed areas. Cells that a fractional clip
    // edge cuts take the polygon path's geometric clip, so every interior cell
    // keeps the kernel's value whichever tile or clip omission produced it.
    if let Some(glyph) = signed::admit(shapes) {
        // The signed-area kernel counts on its own, under the same budget.
        let used = scratch.exact.row.work.used();
        let mut work = used;
        let viewport = (origin, (width, height));
        let result = scratch
            .signed
            .render(glyph, out, viewport, prepared.0, limits, &mut work);
        scratch.signed_bytes = scratch.signed.retained_bytes();
        scratch.exact.row.work.charge(work - used)?;
        if !result? {
            return Ok(());
        }
        let mut cut = Vec::new();
        cut.try_reserve_exact(out.len())
            .map_err(|_| Error::limit("mask allocation failed"))?;
        cut.resize(out.len(), 0);
        polygon(
            width,
            height,
            shapes,
            &mut cut,
            origin,
            limits,
            scratch,
            (prepared, None),
        )?;
        signed::patch(out, &cut, width as usize, origin, prepared.0);
        return Ok(());
    }
    polygon(
        width,
        height,
        shapes,
        out,
        origin,
        limits,
        scratch,
        (prepared, boundary_anchor),
    )
}

// The line sweep or certified integer accumulation, for everything else.
#[expect(
    clippy::too_many_arguments,
    reason = "the coverage entry point's arguments plus its prepared clip"
)]
fn polygon(
    width: u32,
    height: u32,
    shapes: &[Shape],
    out: &mut [u8],
    origin: (u32, u32),
    limits: &Limits,
    scratch: &mut Scratch,
    (prepared, boundary_anchor): ((Rectangle, usize), Option<Point>),
) -> Result<()> {
    if let Some(anchor) =
        boundary_anchor.or_else(|| admission::sweep_anchor(shapes, limits.path_segments))
    {
        let result = scratch.exact.render(
            width,
            shapes,
            out,
            origin,
            limits,
            (prepared.0, prepared.1, anchor),
        );
        scratch.exact_bytes = scratch.exact.retained_bytes();
        result
    } else {
        let work = scratch.exact.row.work.used();
        let result =
            scratch
                .accumulator()?
                .render(shapes, out, origin, (width, height), limits, work);
        scratch.accumulation_bytes =
            bytes(&scratch.accumulation).saturating_add(scratch.accumulation[0].retained_bytes());
        result
    }
}

/// The closed-form rows [`coverage`] would write: every shape is an
/// axis-aligned rectangle, and neither the stroke boundary nor the curve sweep
/// claims them. Validation and work charges equal [`coverage`]'s, including the
/// charge for all `count` mask samples. Other shapes return `None` without
/// charges. The rows take their storage from `samples`.
pub(crate) fn rectangle(
    (width, height): (u32, u32),
    count: usize,
    shapes: &[Shape],
    origin: (u32, u32),
    limits: &Limits,
    scratch: &mut Scratch,
    samples: &mut Vec<u8>,
) -> Result<Option<Rows>> {
    if shapes.is_empty()
        || !shapes.iter().all(|shape| shape.rectangle().is_some())
        || admission::stroke_boundary(shapes, limits.path_segments).is_some()
        || quadratic::anchor(shapes).is_some()
    {
        return Ok(None);
    }
    let (bounds, groups) = scratch
        .exact
        .prepare(shapes, origin, (width, height), limits)?;
    debug_assert_eq!(groups, 0);
    scratch.exact.row.charge(count)?;
    Rows::new(bounds, (width, height), origin, std::mem::take(samples)).map(Some)
}

#[cfg(test)]
pub(crate) fn coverage_accumulated(
    width: u32,
    height: u32,
    shapes: &[Shape],
    out: &mut [u8],
    origin: (u32, u32),
    limits: &Limits,
    scratch: &mut Scratch,
) -> Result<()> {
    // Exercise accumulation even for rectangles and short component contours.
    scratch
        .accumulator()?
        .render(shapes, out, origin, (width, height), limits, 0)
}

#[cfg(test)]
fn coverage_exact(
    width: u32,
    height: u32,
    shapes: &[Shape],
    out: &mut [u8],
    origin: (u32, u32),
    limits: &Limits,
    scratch: &mut Exact,
) -> Result<()> {
    let prepared = scratch.prepare(shapes, origin, (width, height), limits)?;
    scratch.render(
        width,
        shapes,
        out,
        origin,
        limits,
        (prepared.0, prepared.1, Point::new(0.0, 0.0)),
    )
}

impl Exact {
    fn prepare(
        &mut self,
        shapes: &[Shape],
        origin: (u32, u32),
        size: (u32, u32),
        limits: &Limits,
    ) -> Result<(Rectangle, usize)> {
        self.row.work = Meter::new(limits.raster_work, RASTER_WORK);
        self.row.maximum_events = limits.path_segments;
        Rectangle::intersection(shapes, origin, size, &mut self.row)
    }

    fn render(
        &mut self,
        width: u32,
        shapes: &[Shape],
        out: &mut [u8],
        origin: (u32, u32),
        limits: &Limits,
        (bounds, groups, anchor): (Rectangle, usize, Point),
    ) -> Result<()> {
        let stride =
            usize::try_from(width).map_err(|_| Error::limit("mask width exceeds usize"))?;
        if groups == 0 && !shapes.is_empty() {
            self.row.charge(out.len())?;
            return bounds.write(out, stride, origin);
        }
        let bounds = Rectangle {
            left: bounds.left - anchor.x,
            right: bounds.right - anchor.x,
            top: bounds.top - anchor.y,
            bottom: bounds.bottom - anchor.y,
        };
        self.row.reset(stride, origin.0)?;
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
        self.sweep_edges(stride, out, origin, groups, anchor.y)
    }

    fn sweep_edges(
        &mut self,
        stride: usize,
        out: &mut [u8],
        origin: (u32, u32),
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
                + f64::from(origin.1)
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
        .extend(top.iter().enumerate().map(|(i, &e)| (i, e, e.x(hi))));
    row.charge(top.len())?;
    // One bottom-order insertion pass discovers every inversion once. The
    // inserted edge keeps its original top index throughout its exchanges;
    // event owners are therefore monotone, even before sorting by event y.
    for i in 1..sweep.ordered.len() {
        let mut j = i;
        while j > 0 {
            row.charge(1)?;
            let (_, a, ax) = sweep.ordered[j - 1];
            let (owner, b, bx) = sweep.ordered[j];
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
        end = end.max(sweep.ordered[i].0);
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
            for ((_, edge, _), &original) in ordered.iter_mut().zip(block) {
                *edge = original;
            }
            crossing(ordered, current, lo, hi, winding, row, &mut sweep.winding)?;
            for &edge in block {
                winding.step(edge)?;
            }
        }
        start = i + 1;
    }
    // Each block now retains its last interval's order for the next strip.
    for (edge, &(_, ordered, _)) in top.iter_mut().zip(&sweep.ordered) {
        *edge = ordered;
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
    ordered: &mut [(usize, Edge, f64)],
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
        for (_, edge, x) in ordered.iter_mut() {
            *x = edge.x(mid);
        }
        repair(
            ordered,
            |(_, a, x), (_, b, y)| cached(*a, *x, *b, *y, false),
            row,
        )?;
        winding.values.clone_from(&initial.values);
        winding.outside = initial.outside;
        for &(_, edge, _) in ordered.iter() {
            row.edge(edge, start, end, winding.step(edge)?)?;
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
                clipped(
                    edges,
                    previous,
                    next,
                    group,
                    rule,
                    [bounds.left, bounds.top, bounds.right, bounds.bottom],
                    limits,
                )?;
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
    for (group, x, y, delta, rule) in events.drain(..) {
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

#[cfg(test)]
mod glyph_tests;
