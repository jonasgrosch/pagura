//! Approximate signed cells: the `approximate_coverage` fast parameter.
//!
//! Curves become 1/64-pixel chords and each edge adds its signed area to
//! f32 cells, row by row, in the order of its contour; the running sum
//! folds to coverage without resolving overlaps. Compound paths that no
//! stroke boundary or fill certificate describes, rectangles and geometric
//! points keep exact coverage. A clip other than quadrilaterals around a
//! quadrilateral mark multiplies in: the product of its paths' own
//! coverages, then its rectangles' column times row factor. None of this is
//! within the one-level contract; every cell is still a function of the
//! geometry and the cell alone.

use super::DeviceRect;
use super::clip::{ClipShape, fill_polygon};
use super::edges::{self, Bounds, Line, Polygon, RowList, rows};
use super::raster::Out;
use super::row::Sink;
use super::shape::{Body, Shape};
use super::{Area, BandCoverage, Precision, Prep};
use crate::{FillRule, Path, Point, Segment, Transform};
use pagura_core::{Error, Meter, Result};

/// The chord tolerance of approximate cells, in device pixels.
pub(crate) const TOLERANCE: f64 = 1.0 / 64.0;

/// A shape's edges for approximate cells, in contour order.
#[derive(Debug)]
pub(crate) struct Cells {
    lines: Vec<Line>,
    index: RowList,
    scan: (usize, usize),
}

/// Reusable f32 cells of one row.
#[derive(Debug, Default)]
pub(crate) struct Scratch {
    values: Vec<f32>,
    touched: Vec<u64>,
    product: Vec<u8>,
}

impl Scratch {
    pub(crate) fn new(width: usize, bytes: &mut Meter) -> Result<Self> {
        let mut scratch = Self::default();
        bytes.reserve(&mut scratch.values, width)?;
        bytes.reserve(&mut scratch.touched, width.div_ceil(64) + 1)?;
        bytes.reserve(&mut scratch.product, width)?;
        scratch.values.resize(width, 0.0);
        scratch.touched.resize(width.div_ceil(64) + 1, 0);
        Ok(scratch)
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.values
            .capacity()
            .saturating_mul(4)
            .saturating_add(self.touched.capacity().saturating_mul(8))
            .saturating_add(self.product.capacity())
    }

    #[inline]
    fn delta(&mut self, x: usize, value: f32) {
        if value != 0.0
            && let Some(cell) = self.values.get_mut(x)
        {
            *cell += value;
            if let Some(word) = self.touched.get_mut(x / 64) {
                *word |= 1 << (x % 64);
            }
        }
    }

    /// Adds the part of `line` within row `top..top + 1`. Returns the cells
    /// visited.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "f32 cells by definition; columns clamped to the row"
    )]
    fn edge(&mut self, Line { a, b }: Line, top: f64) -> usize {
        let (a, b, sign) = if a.y < b.y { (a, b, 1.0) } else { (b, a, -1.0) };
        let low = a.y.max(top);
        let high = b.y.min(top + 1.0);
        if high <= low {
            return 0;
        }
        let dx = (b.x - a.x) / (b.y - a.y);
        let ax = a.x + (low - a.y) * dx;
        let bx = a.x + (high - a.y) * dx;
        let right_edge = self.values.len() as f64;
        if ax == bx {
            if ax < 0.0 {
                self.delta(0, ((high - low) * sign) as f32);
            } else if ax < right_edge {
                let column = ax.floor();
                let x = column as usize;
                let area = (high - low) * sign;
                let fraction = ax - column;
                self.delta(x, (area * (1.0 - fraction)) as f32);
                self.delta(x + 1, (area * fraction) as f32);
            }
            return 2;
        }
        let (left, right) = (ax.min(bx), ax.max(bx));
        if left < 0.0 {
            let covered = if right <= 0.0 {
                high - low
            } else {
                (0.0 - left) / (right - left) * (high - low)
            };
            self.delta(0, (covered * sign) as f32);
        }
        let from = left.floor().clamp(0.0, right_edge);
        let to = right.ceil().clamp(0.0, right_edge);
        let rate = (high - low) / (right - left) * sign;
        let mut column = from;
        let mut count = 1;
        while column < to {
            let x = column as usize;
            let l = left.max(column);
            let r = right.min(column + 1.0);
            let cover = if rate.is_finite() {
                (r - l) * rate
            } else {
                (r - l) / (right - left) * (high - low) * sign
            };
            let area = cover * (1.0 - f64::midpoint(l - column, r - column));
            self.delta(x, area as f32);
            self.delta(x + 1, (cover - area) as f32);
            column += 1.0;
            count += 1;
        }
        count
    }

    /// The row buffer multiplied clips scale coverage in.
    pub(crate) fn row_buffer(&mut self) -> &mut Vec<u8> {
        &mut self.product
    }

    /// Rasterizes `cells` of `shape` in `rows` of `band` into `out`.
    pub(crate) fn rasterize(
        &mut self,
        cells: &Cells,
        shape: &Shape,
        (band, rows): (u32, std::ops::Range<u32>),
        out: &mut BandCoverage,
    ) -> usize {
        let _ = band;
        let mut work = 0;
        for y in rows {
            let top = f64::from(y);
            for &item in cells.index.row(y) {
                if let Some(&line) = cells.lines.get(item as usize) {
                    work += self.edge(line, top) + 1;
                }
            }
            out.push_row(y);
            work += self.emit(cells.scan, shape, y, out);
        }
        work
    }

    /// Folds the running sum of row `y`'s cells into coverage, clears them,
    /// and writes it scaled by the shape's multiplied clip. Returns the
    /// work spent.
    fn emit(
        &mut self,
        (from, to): (usize, usize),
        shape: &Shape,
        y: u32,
        out: &mut BandCoverage,
    ) -> usize {
        let to = to.min(self.values.len());
        let rule = shape.rule;
        let mut sink = Multiplied::new(out, shape, y, &mut self.product);
        let mut work = 2;
        if from < to {
            let mut winding = 0.0_f32;
            let mut left = from;
            for word in from / 64..=(to - 1) / 64 {
                let mut bits = std::mem::take(&mut self.touched[word]);
                work += 1;
                while bits != 0 {
                    let x = word * 64 + bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    let value = std::mem::take(&mut self.values[x]);
                    if x >= to {
                        continue;
                    }
                    if left < x {
                        sink.run(left, x, fold(winding, rule));
                    }
                    winding += value;
                    sink.run(x, x + 1, fold(winding, rule));
                    left = x + 1;
                    work += 3;
                }
            }
            if left < to {
                sink.run(left, to, fold(winding, rule));
            }
        }
        work + sink.work
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "folded area is clamped to 0..1 before byte rounding"
)]
fn fold(winding: f32, rule: FillRule) -> u8 {
    let area = match rule {
        FillRule::NonZero => winding.abs().min(1.0),
        FillRule::EvenOdd => {
            let t = winding.abs() % 2.0;
            t.min(2.0 - t)
        }
    };
    (area * 255.0).round() as u8
}

/// `round(a × b / 255)`.
#[expect(clippy::cast_possible_truncation, reason = "a rounded byte product")]
fn product(a: u8, b: u8) -> u8 {
    ((u16::from(a) * u16::from(b) + 127) / 255) as u8
}

/// A coverage sink that scales runs by a multiplied clip: the product of
/// its paths, then its rectangle's column and row factors.
pub(crate) struct Multiplied<'a> {
    out: Out<'a>,
    clip: Option<&'a ClipShape>,
    y: u32,
    row: &'a mut Vec<u8>,
    work: usize,
}

impl<'a> Multiplied<'a> {
    pub(crate) fn new(
        out: &'a mut BandCoverage,
        shape: &'a Shape,
        y: u32,
        row: &'a mut Vec<u8>,
    ) -> Self {
        let clip = shape.multiplied.as_ref();
        Self {
            out: Out {
                coverage: out,
                x0: shape.bounds.x0 as usize,
                x1: shape.bounds.x1 as usize,
            },
            clip,
            y,
            row,
            work: 0,
        }
    }
}

impl Multiplied<'_> {
    pub(crate) fn work(&self) -> usize {
        self.work
    }
}

impl Sink for Multiplied<'_> {
    fn run(&mut self, x0: usize, x1: usize, value: u8) {
        let Some(clip) = self.clip else {
            self.out.run(x0, x1, value);
            return;
        };
        let a = x0.max(self.out.x0);
        let b = x1.min(self.out.x1);
        if a >= b || value == 0 {
            return;
        }
        self.work += b - a;
        self.row.clear();
        self.row.extend(std::iter::repeat_n(value, b - a));
        if let Some(product) = clip.product() {
            if let Some(factors) = product.row(self.y, a, b) {
                for (v, &f) in self.row.iter_mut().zip(factors) {
                    *v = product_or(*v, f);
                }
            }
        } else if let Some(rows) = clip.rows() {
            rows.scale(self.y, a, self.row);
        }
        if let Some(rect) = clip.rect() {
            scale_edges(rect, (a, self.y), self.row);
        }
        for (x, &v) in (a..).zip(self.row.iter()) {
            self.out.run(x, x + 1, v);
        }
    }
}

fn product_or(value: u8, factor: u8) -> u8 {
    if factor == u8::MAX {
        value
    } else {
        product(value, factor)
    }
}

/// Scales a coverage row from device `(x0, y)` by `rect`'s coverage there:
/// a column factor times a row factor, rounding each product once.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "factors in [0, 1] scale to bytes; device columns fit f64"
)]
pub(crate) fn scale_edges(rect: Bounds, (x0, y): (usize, u32), row: &mut [u8]) {
    let overlap =
        |start: f64, low: f64, high: f64| (high.min(start + 1.0) - low.max(start)).clamp(0.0, 1.0);
    let vertical = overlap(f64::from(y), rect.top, rect.bottom);
    for (column, value) in (x0..).zip(row.iter_mut()) {
        let horizontal = overlap(column as f64, rect.left, rect.right);
        if vertical == 1.0 && horizontal == 1.0 {
            continue;
        }
        let level = (horizontal * vertical * 255.0).round() as u16;
        *value = ((u16::from(*value) * level + 127) / 255) as u8;
    }
}

/// The product of a clip's non-rectangular paths' own coverages over its
/// bounds, row by row.
#[derive(Debug)]
pub(crate) struct Product {
    bounds: DeviceRect,
    values: Vec<u8>,
    work: usize,
    retained: usize,
}

impl Product {
    /// The product of the coverages of `paths` that are not axis-aligned
    /// rectangles, each rasterized as an unclipped fill under the
    /// approximate rules, over `bounds`.
    pub(crate) fn new(
        paths: &[(&Path, FillRule)],
        transform: Transform,
        options: &Prep<'_>,
        bounds: DeviceRect,
    ) -> Result<Self> {
        let count = (bounds.width() as usize).saturating_mul(bounds.height() as usize);
        let mut meter = Meter::new(
            options.limits.decoded_bytes,
            "clip product allocation failed",
        );
        let mut values = Vec::new();
        meter.reserve(&mut values, count)?;
        values.resize(count, u8::MAX);
        let area = Area {
            region: bounds,
            band_height: options.area.band_height.max(1),
        };
        let mut bytes = Meter::new(usize::MAX, "clip product allocation failed");
        let mut raster = super::BandRasterizer::new(area, &mut bytes)?;
        let mut band = BandCoverage::default();
        let mut work = count;
        for &(path, rule) in paths {
            let (polygon, _) = fill_polygon(path, transform, rule, None, options.limits)?;
            if super::rect::rectangle(&polygon).is_some() {
                continue;
            }
            let unclipped = Prep {
                area,
                precision: Precision::Approximate,
                limits: options.limits,
                clip: None,
            };
            let Some(shape) = Shape::fill_in(path, transform, rule, &unclipped)? else {
                values.fill(0);
                continue;
            };
            work = work.saturating_add(shape.work()).saturating_add(count);
            let mut coverage = vec![0_u8; count];
            for b in shape.bands(&area) {
                raster.rasterize(
                    super::MarkCoverage {
                        shape: Some(&shape),
                        clip: None,
                    },
                    b,
                    &mut band,
                );
                write(&band, bounds, &mut coverage);
            }
            for (v, &c) in values.iter_mut().zip(&coverage) {
                *v = product(*v, c);
            }
        }
        Ok(Self {
            bounds,
            retained: values.capacity(),
            values,
            work,
        })
    }

    /// The factors of row `y`, columns `x0..x1`, if the product covers them.
    fn row(&self, y: u32, x0: usize, x1: usize) -> Option<&[u8]> {
        let b = self.bounds;
        if y < b.y0 || y >= b.y1 || x0 < b.x0 as usize || x1 > b.x1 as usize {
            return None;
        }
        let start = (y - b.y0) as usize * b.width() as usize + x0 - b.x0 as usize;
        self.values.get(start..start + (x1 - x0))
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.retained
    }

    pub(crate) fn work(&self) -> usize {
        self.work
    }

    /// Writes the product of row `y` scaled by `rect` to `sink`, within
    /// columns `x0..x1`. Returns the pixels written.
    pub(crate) fn emit_row(
        &self,
        y: u32,
        rect: Option<Bounds>,
        (x0, x1): (usize, usize),
        sink: &mut impl Sink,
    ) -> usize {
        let Some(row) = self.row(y, x0, x1) else {
            return 0;
        };
        let mut scaled = [0_u8; 64];
        for (start, chunk) in (x0..).step_by(64).zip(row.chunks(64)) {
            let scaled = &mut scaled[..chunk.len()];
            scaled.copy_from_slice(chunk);
            if let Some(rect) = rect {
                scale_edges(rect, (start, y), scaled);
            }
            for (x, &v) in (start..).zip(scaled.iter()) {
                sink.run(x, x + 1, v);
            }
        }
        row.len()
    }
}

/// Writes `band`'s spans into `out`, a row-major image of `bounds`.
fn write(band: &BandCoverage, bounds: DeviceRect, out: &mut [u8]) {
    let width = bounds.width() as usize;
    for row in band.rows() {
        let line = (row.y - bounds.y0) as usize * width;
        for span in row.spans() {
            match span {
                super::Span::Full { x, len } => {
                    let start = line + (x - bounds.x0) as usize;
                    if let Some(run) = out.get_mut(start..start + len as usize) {
                        run.fill(u8::MAX);
                    }
                }
                super::Span::Partial { x, coverage } => {
                    let start = line + (x - bounds.x0) as usize;
                    if let Some(run) = out.get_mut(start..start + coverage.len()) {
                        run.copy_from_slice(coverage);
                    }
                }
            }
        }
    }
}

/// A fill under the approximate rules.
pub(crate) fn fill(
    path: &Path,
    transform: Transform,
    rule: FillRule,
    options: &Prep<'_>,
) -> Result<Option<Shape>> {
    let limits = options.limits;
    if path.segments().len() > limits.path_segments {
        return Err(Error::limit("path segment budget exceeded"));
    }
    let empty = path.fill_region(rule) == Some(crate::FillRegion::EmptyBoundary);
    let compound = path.fill_region(rule).is_none() && subpaths(path) >= 2;
    let quad = quadrilateral(path, transform);
    let hull = {
        let [min, max] = crate::bounds::control_hull(path, transform, limits)?;
        (min.x <= max.x && min.y <= max.y).then_some([
            Point::new(min.x.floor(), min.y.floor()),
            Point::new(max.x.floor() + 1.0, max.y.floor() + 1.0),
        ])
    };
    let exact = empty || compound || quad.is_some_and(|q| axis_aligned(q).is_some());
    let cells = if exact {
        None
    } else {
        Some(Polygon::from_path(
            path,
            transform,
            Some(TOLERANCE),
            limits,
        )?)
    };
    place(
        &Source {
            cells,
            hull,
            quad,
            exact: &|options| {
                let (polygon, rule) = fill_polygon(path, transform, rule, None, options.limits)?;
                Shape::polygon(polygon, rule, super::shape::Winding::Check(0.0), options)
            },
        },
        rule,
        options,
    )
}

/// A stroke outline under the approximate rules: pieces that meet without
/// overlapping (or a run's convex boundaries) take cells.
pub(crate) fn stroke(
    outline: &Path,
    transform: Transform,
    pieces: bool,
    options: &Prep<'_>,
) -> Result<Option<Shape>> {
    let limits = options.limits;
    let source = if pieces {
        Some(outline)
    } else {
        outline.boundary.as_deref()
    };
    let compound = source.is_none() && subpaths(outline) >= 2;
    let quad = quadrilateral(outline, transform);
    let hull = {
        let [min, max] = crate::bounds::control_hull(outline, transform, limits)?;
        (min.x <= max.x && min.y <= max.y).then_some([
            Point::new(min.x.floor(), min.y.floor()),
            Point::new(max.x.floor() + 1.0, max.y.floor() + 1.0),
        ])
    };
    let exact = compound || quad.is_some_and(|q| axis_aligned(q).is_some());
    let cells = if exact {
        None
    } else {
        Some(Polygon::from_path(
            source.unwrap_or(outline),
            transform,
            Some(TOLERANCE),
            limits,
        )?)
    };
    place(
        &Source {
            cells,
            hull,
            quad,
            exact: &|options| {
                let polygon = Polygon::from_path(outline, transform, None, options.limits)?;
                let winding = if pieces {
                    super::shape::Winding::Certified
                } else {
                    super::shape::Winding::Unknown
                };
                Shape::polygon(polygon, FillRule::NonZero, winding, options)
            },
        },
        FillRule::NonZero,
        options,
    )
}

/// A placed glyph polygon under the approximate rules.
pub(crate) fn glyph(polygon: &Polygon, options: &Prep<'_>) -> Result<Option<Shape>> {
    let quad = polygon_quadrilateral(polygon);
    let hull = if polygon.points_cells {
        None
    } else {
        polygon.bounds()
    };
    let exact = polygon.points_cells || quad.is_some_and(|q| axis_aligned(q).is_some());
    let cells = (!exact).then(|| polygon.clone_shallow());
    place(
        &Source {
            cells,
            hull,
            quad,
            exact: &|options| {
                Shape::polygon(
                    polygon.clone_shallow(),
                    FillRule::NonZero,
                    super::shape::Winding::Check(0.0),
                    options,
                )
            },
        },
        FillRule::NonZero,
        options,
    )
}

/// What an approximate mark offers: its cells' polygon, unless it keeps
/// exact coverage, its pixel hull, its quadrilateral, and its exact shape.
struct Source<'a> {
    cells: Option<Polygon>,
    hull: Option<[Point; 2]>,
    quad: Option<[Point; 4]>,
    exact: &'a dyn Fn(&Prep<'_>) -> Result<Option<Shape>>,
}

/// Chooses how a mark meets its clip: dropped where every path of it holds
/// the mark, integrated geometrically for a quadrilateral within
/// quadrilaterals, and multiplied otherwise.
fn place(source: &Source<'_>, rule: FillRule, options: &Prep<'_>) -> Result<Option<Shape>> {
    let unclipped = Prep {
        precision: Precision::Exact,
        clip: None,
        ..*options
    };
    let Some(clip) = options.clip else {
        return mark(source, rule, &unclipped);
    };
    let needed: Vec<&ClipPath> = clip
        .paths()
        .iter()
        .filter(|p| {
            !(source.hull.is_some_and(|hull| p.contains_hull(hull))
                || source.hull.is_some_and(|hull| p.covers(hull)))
        })
        .collect();
    if needed.is_empty() {
        return mark(source, rule, &unclipped);
    }
    if source.quad.is_some() && needed.iter().all(|p| p.quad.is_some()) {
        let exact = Prep {
            precision: Precision::Exact,
            ..*options
        };
        return (source.exact)(&exact);
    }
    let Some(mut shape) = mark(source, rule, &unclipped)? else {
        return Ok(None);
    };
    let limit = clip.bounds();
    shape.bounds = shape.bounds.intersect(&limit);
    if shape.bounds.is_empty() {
        return Ok(None);
    }
    shape.multiplied = Some(clip.clone());
    shape.add_work((shape.bounds.width() as usize).saturating_mul(shape.bounds.height() as usize));
    Ok(Some(shape))
}

/// The mark alone: approximate cells where it takes them, exact otherwise.
fn mark(source: &Source<'_>, rule: FillRule, options: &Prep<'_>) -> Result<Option<Shape>> {
    match &source.cells {
        Some(polygon) => cells(polygon, rule, options),
        None => (source.exact)(options),
    }
}

/// A shape of approximate cells of `polygon`.
fn cells(polygon: &Polygon, rule: FillRule, options: &Prep<'_>) -> Result<Option<Shape>> {
    let area = options.area;
    let region = area.region;
    let Some(bounds) = polygon.bounds() else {
        return Ok(None);
    };
    let bounds_px = edges::pixels(bounds, region, false);
    if bounds_px.is_empty() {
        return Ok(None);
    }
    let mut lines = Vec::new();
    lines
        .try_reserve(polygon.vertices())
        .map_err(|_| Error::limit("raster geometry allocation failed"))?;
    for contour in polygon.contours() {
        let n = contour.len();
        for i in 0..n {
            let (a, b) = (contour[i], contour[(i + 1) % n]);
            if a.y != b.y {
                if !((b.x - a.x) / (b.y - a.y)).is_finite() {
                    return Err(Error::limit("cell slope exceeds numeric range"));
                }
                lines.push(Line { a, b });
            }
        }
    }
    let right = f64::from(region.x1);
    let index = RowList::new((bounds_px.y0, bounds_px.y1), lines.len(), |i| {
        let l = lines.get(i)?;
        if l.a.x.min(l.b.x) >= right {
            return None;
        }
        rows(l.a.y.min(l.b.y), l.a.y.max(l.b.y), bounds_px)
    })?;
    let width = super::row::width(region.x1)?;
    let scan = super::clip::columns(bounds[0].x, bounds[1].x, width);
    let mut work = index.len();
    for l in &lines {
        if let Some(r) = rows(l.a.y.min(l.b.y), l.a.y.max(l.b.y), bounds_px) {
            work = work.saturating_add(edges::line_work(*l, r, width).saturating_mul(2));
        }
    }
    let rows = bounds_px.height() as usize;
    work = work.saturating_add(rows.saturating_mul((scan.1 - scan.0) / 64 + 4));
    work = work.saturating_add(work);
    if work > options.limits.raster_work {
        return Err(Error::limit(super::shape::RASTER_WORK));
    }
    let retained = lines.capacity() * size_of::<Line>() + index.retained_bytes();
    Ok(Some(Shape::with_body(
        Body::Cells(Cells { lines, index, scan }),
        bounds_px,
        (rule, work, retained),
        options,
    )))
}

fn subpaths(path: &Path) -> usize {
    path.segments()
        .iter()
        .filter(|s| matches!(s, Segment::MoveTo(_)))
        .take(2)
        .count()
}

/// The device corners of a path of one closed four-sided contour.
pub(crate) fn quadrilateral(path: &Path, transform: Transform) -> Option<[Point; 4]> {
    use Segment::{Close, LineTo, MoveTo};
    let [MoveTo(a), LineTo(b), LineTo(c), LineTo(d), tail @ ..] = path.segments() else {
        return None;
    };
    if !matches!(tail, [] | [Close])
        && !matches!(tail, [LineTo(end)] | [LineTo(end), Close] if end == a)
    {
        return None;
    }
    if !transform.is_finite() {
        return None;
    }
    let points = [*a, *b, *c, *d];
    let mapped = points.map(|p| p * transform);
    if points
        .into_iter()
        .chain(mapped)
        .any(|p| crate::flatten::bounded(p).is_err())
    {
        return None;
    }
    Some(mapped)
}

fn polygon_quadrilateral(polygon: &Polygon) -> Option<[Point; 4]> {
    let mut contours = polygon.contours();
    match (contours.next(), contours.next()) {
        (Some(&[a, b, c, d]), None) => Some([a, b, c, d]),
        _ => None,
    }
}

/// The rectangle of an axis-aligned quadrilateral.
pub(crate) fn axis_aligned([a, b, c, d]: [Point; 4]) -> Option<Bounds> {
    let axis = (a.x == b.x && b.y == c.y && c.x == d.x && d.y == a.y)
        || (a.y == b.y && b.x == c.x && c.y == d.y && d.x == a.x);
    (axis && a != b && b != c && c != d && d != a).then_some(Bounds {
        left: a.x.min(c.x),
        top: a.y.min(c.y),
        right: a.x.max(c.x),
        bottom: a.y.max(c.y),
    })
}

/// One path of a clip, as the approximate rules see it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClipPath {
    pub(crate) quad: Option<[Point; 4]>,
    pub(crate) rect: Option<Bounds>,
}

impl ClipPath {
    pub(crate) fn new(path: &Path, transform: Transform) -> Self {
        let quad = quadrilateral(path, transform);
        Self {
            quad,
            rect: quad.and_then(axis_aligned),
        }
    }

    /// Whether the path is a rectangle holding all of `hull`.
    fn covers(&self, [lo, hi]: [Point; 2]) -> bool {
        self.rect.is_some_and(|r| {
            r.left <= lo.x.floor()
                && r.top <= lo.y.floor()
                && r.right >= hi.x.ceil()
                && r.bottom >= hi.y.ceil()
        })
    }

    /// Strict convex half-planes contain the entire hull. Outward-rounded
    /// orientation intervals decline near-boundary cases instead of
    /// guessing.
    fn contains_hull(&self, [lo, hi]: [Point; 2]) -> bool {
        let Some(points) = self.quad else {
            return false;
        };
        let Some(sign) = orientation(points[0], points[1], points[2]) else {
            return false;
        };
        let corners = [lo, Point::new(hi.x, lo.y), hi, Point::new(lo.x, hi.y)];
        (0..4).all(|i| {
            orientation(points[i], points[(i + 1) % 4], points[(i + 2) % 4]) == Some(sign)
                && corners
                    .iter()
                    .all(|&p| orientation(points[i], points[(i + 1) % 4], p) == Some(sign))
        })
    }
}

fn orientation(a: Point, b: Point, c: Point) -> Option<i8> {
    let difference = |x: f64, y: f64| {
        let value = x - y;
        [value.next_down(), value.next_up()]
    };
    let product = |a: [f64; 2], b: [f64; 2]| {
        let values = [a[0] * b[0], a[0] * b[1], a[1] * b[0], a[1] * b[1]];
        [
            values.into_iter().fold(f64::INFINITY, f64::min).next_down(),
            values
                .into_iter()
                .fold(f64::NEG_INFINITY, f64::max)
                .next_up(),
        ]
    };
    let left = product(difference(b.x, a.x), difference(c.y, a.y));
    let right = product(difference(b.y, a.y), difference(c.x, a.x));
    if (left[0] - right[1]).next_down() > 0.0 {
        Some(1)
    } else if (left[1] - right[0]).next_up() < 0.0 {
        Some(-1)
    } else {
        None
    }
}

impl Polygon {
    /// A copy of the contours.
    pub(crate) fn clone_shallow(&self) -> Self {
        Self {
            points: self.points.clone(),
            starts: self.starts.clone(),
            points_cells: self.points_cells,
        }
    }
}
