//! Bounded adaptive subdivision in device space.

use crate::geometry::{greater, lesser};
use crate::{Path, Point, Segment, Transform};
use pagura_core::{Error, Limits, Meter, Result};

/// Maximum distance of a flattened control point from its chord, in pixels.
/// The convex-hull property bounds the whole curve by this tolerance.
pub(crate) const TOLERANCE: f64 = 1.0 / 256.0;

/// Largest coordinate magnitude, in user or device space, that geometry may
/// have. It keeps intermediate products and intersections finite; larger
/// finite coordinates are a resource limit, not malformed geometry.
pub(crate) const MAX_COORDINATE: f64 = 1e12;

/// Subdivision depth at which a curve counts as non-converging. Each
/// halving at least quarters a cubic's control-point distance from its
/// chord, so curves within [`MAX_COORDINATE`] reach [`TOLERANCE`] after
/// about 25 levels; this bound is reached only through rounding.
const MAX_SUBDIVISION_DEPTH: usize = 64;

/// The approximation bound for one curve: the canonical distance, or a
/// share of a whole-outline area allowance. A curve and its chord lie in
/// the same control hull, so that hull's area bounds their symmetric
/// difference even when the outline overlaps or a clip cuts it.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Flatness {
    Distance,
    Area(f64),
}

impl Flatness {
    fn accepts(self, controls: [Point; 4], depth: usize, rounding: f64, near: &Near) -> bool {
        match self {
            Self::Distance => near.flat(controls),
            Self::Area(allowance) => {
                let share = allowance * 0.5_f64.powi(i32::try_from(depth).unwrap_or(i32::MAX));
                Hull::new(controls, rounding).area <= share
            }
        }
    }
}

/// A curve piece and its chord lie in this outward control hull. The
/// three triangles cover that hull; determinant error includes mapping,
/// subdivision and cancellation, so an uncertain zero never means zero area.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Hull {
    pub(crate) bounds: [f64; 4],
    pub(crate) area: f64,
}

impl Hull {
    fn new([a, b, c, d]: [Point; 4], rounding: f64) -> Self {
        let triangle = |p: Point, q: Point| {
            let (ux, uy, vx, vy) = (p.x - a.x, p.y - a.y, q.x - a.x, q.y - a.y);
            let products = (ux * vy).abs() + (uy * vx).abs();
            let radius = 2.0 * rounding;
            let perturbation =
                radius * (ux.abs() + uy.abs() + vx.abs() + vy.abs()) + 2.0 * radius * radius;
            let determinant =
                (ux * vy - uy * vx).abs() + 8.0 * f64::EPSILON * products + perturbation;
            determinant * (0.5 + 8.0 * f64::EPSILON) + f64::MIN_POSITIVE
        };
        Self {
            bounds: [
                (lesser(lesser(a.x, b.x), lesser(c.x, d.x)) - rounding).next_down(),
                (lesser(lesser(a.y, b.y), lesser(c.y, d.y)) - rounding).next_down(),
                (greater(greater(a.x, b.x), greater(c.x, d.x)) + rounding).next_up(),
                (greater(greater(a.y, b.y), greater(c.y, d.y)) + rounding).next_up(),
            ],
            area: (triangle(b, c) + triangle(b, d) + triangle(c, d)) * (1.0 + 8.0 * f64::EPSILON),
        }
    }

    pub(crate) fn intersects(self, b: [f64; 4]) -> bool {
        let a = self.bounds;
        b[0] < b[2] && b[1] < b[3] && a[0] < b[2] && a[1] < b[3] && a[2] > b[0] && a[3] > b[1]
    }
}

/// Optional original-curve evidence from the same subdivision used for
/// polygon production. Refinement changes only pieces whose full outward
/// hull meets a target rectangle after `offset` is added.
/// When hulls are retained, only those targets need the area bound: a
/// disjoint hull may keep its chord, since moving the curve to that chord
/// stays outside every target. Without retained hulls, the whole outline
/// keeps its area allowance, including outside the refinement targets.
pub(crate) struct Inspection<'a> {
    pub(crate) hulls: Option<&'a mut Vec<Hull>>,
    pub(crate) work: &'a mut Meter,
    pub(crate) rounding: f64,
    pub(crate) targets: &'a [[f64; 4]],
    pub(crate) offset: Point,
    pub(crate) factor: f64,
}

/// Pending curve pieces: controls, the device position of the end and the
/// subdivision depth.
pub(crate) type CurveStack = Vec<([Point; 4], Point, usize)>;

/// Source evidence needed by the consumer; fills and solid strokes never
/// retain the additional controls used to position interior dash cuts.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Fill,
    Stroke,
    Dashed,
}

/// The inner controls of one accepted cubic, whose endpoints remain in
/// `Polyline::points[index..=index + 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CurvePiece {
    pub(crate) index: usize,
    pub(crate) controls: [Point; 2],
}

/// A flattened contour with source closure and geometric-point metadata.
#[derive(Debug, PartialEq)]
pub(crate) struct Polyline {
    pub(crate) points: Vec<Point>,
    pub(crate) closed: bool,
    pub(crate) degenerate: bool,
    /// Forward source endpoint directions, also retained through degenerate segments.
    pub(crate) tangents: [Option<Point>; 2],
    pub(crate) curves: [bool; 2],
    /// Sparse source joins; subdivision points retain their chord directions.
    pub(crate) joins: Vec<Join>,
    pub(crate) pieces: Vec<CurvePiece>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Join {
    pub(crate) index: usize,
    pub(crate) tangents: [Point; 2],
}

impl Polyline {
    pub(crate) fn heap_bytes(&self) -> usize {
        self.points
            .capacity()
            .saturating_mul(size_of::<Point>())
            .saturating_add(self.joins.capacity().saturating_mul(size_of::<Join>()))
            .saturating_add(
                self.pieces
                    .capacity()
                    .saturating_mul(size_of::<CurvePiece>()),
            )
    }

    fn join_to(&mut self, direction: Point, bytes: &mut Meter) -> Result<()> {
        let before = self.tangents[1];
        if let Some(before) = before
            && direction.length() > 0.0
        {
            self.add_join(
                self.points.len() - 1,
                [before, direction / direction.length()],
                bytes,
            )?;
        }
        Ok(())
    }

    fn add_join(&mut self, index: usize, tangents: [Point; 2], bytes: &mut Meter) -> Result<()> {
        bytes.reserve_capacity(&mut self.joins, 1)?;
        let join = Join { index, tangents };
        if index == 0 {
            self.joins.insert(0, join);
        } else {
            self.joins.push(join);
        }
        Ok(())
    }

    fn close_joins(&mut self, bytes: &mut Meter) -> Result<()> {
        let Some((&first, rest)) = self.points.split_first() else {
            return Ok(());
        };
        let Some(&last) = rest.last() else {
            return Ok(());
        };
        let after = self.tangents[0];
        let before = if first == last {
            self.tangents[1]
        } else {
            Some((first - last) / (first - last).length())
        };
        if first != last && self.curves[1] {
            self.join_to(first - last, bytes)?;
        }
        if (self.curves[0] || first == last && self.curves[1])
            && let (Some(before), Some(after)) = (before, after)
        {
            self.add_join(0, [before, after], bytes)?;
            if first == last {
                self.add_join(self.points.len() - 1, [before, after], bytes)?;
            }
        }
        Ok(())
    }

    fn record_tangents(
        &mut self,
        from: Point,
        [c1, c2, end]: [Point; 3],
        curve: bool,
        joins: Option<&mut Meter>,
    ) -> Result<()> {
        let direction = |origin: Point, candidates: [Point; 3]| {
            candidates
                .into_iter()
                .find(|&point| point != origin)
                .map(|point| (point - origin) / (point - origin).length())
        };
        if let Some(start) = direction(from, [c1, c2, end]) {
            if (curve || self.curves[1])
                && let Some(bytes) = joins
            {
                self.join_to(start, bytes)?;
            }
            if self.degenerate {
                self.tangents[0] = Some(start);
                self.curves[0] = curve;
            }
            if let Some(back) = direction(end, [c2, c1, from]) {
                self.tangents[1] = Some(-back);
                self.curves[1] = curve;
            }
        }
        Ok(())
    }

    pub(crate) fn strokeable(&self, cap: crate::LineCap) -> bool {
        if self.points.len() < 2 && !self.closed {
            return false;
        }
        self.points.windows(2).any(|p| p[0] != p[1]) || cap == crate::LineCap::Round
    }
}

/// Heap storage of flattened contours, including their point buffers.
pub(crate) fn retained_bytes(contours: &Vec<Polyline>) -> usize {
    contours
        .capacity()
        .saturating_mul(size_of::<Polyline>())
        .saturating_add(
            contours
                .iter()
                .fold(0usize, |n, c| n.saturating_add(c.heap_bytes())),
        )
}

/// Heap storage of the subdivision stack.
pub(crate) fn stack_bytes(stack: &CurveStack) -> usize {
    stack
        .capacity()
        .saturating_mul(size_of::<([Point; 4], Point, usize)>())
}

/// Checks a point against the working range ([`MAX_COORDINATE`]).
pub(crate) fn bounded(point: Point) -> Result<()> {
    if !point.is_finite() {
        Err(Error::invalid_argument("path geometry is not finite"))
    } else if point.x.abs().max(point.y.abs()) > MAX_COORDINATE {
        Err(Error::limit("geometry exceeds numeric working range"))
    } else {
        Ok(())
    }
}

/// The checks every entry point applies to a caller's path and transform
/// before any work: a finite transform and the segment budget.
pub(crate) fn validate(path: &Path, transform: Transform, limits: &Limits) -> Result<()> {
    if !transform.is_finite() {
        return Err(Error::invalid_argument("path transform is not finite"));
    }
    if path.segments().len() > limits.path_segments {
        return Err(Error::limit("path segment budget exceeded"));
    }
    Ok(())
}

pub(crate) fn flatten(
    path: &Path,
    transform: Transform,
    source: Source,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<Vec<Polyline>> {
    let mut result = Vec::new();
    let mut pending = CurveStack::new();
    let count = flatten_using(
        path,
        transform,
        limits,
        &mut result,
        &mut pending,
        Subdivision {
            flatness: Flatness::Distance,
            bytes,
            inspection: None,
            source,
        },
    )?;
    let scratch = stack_bytes(&pending);
    drop(pending);
    bytes.refund(scratch);
    result.truncate(count);
    Ok(result)
}

pub(crate) fn flatten_into(
    path: &Path,
    transform: Transform,
    limits: &Limits,
    result: &mut Vec<Polyline>,
    pending: &mut CurveStack,
    bytes: &mut Meter,
) -> Result<usize> {
    flatten_with(
        path,
        transform,
        limits,
        result,
        pending,
        Flatness::Distance,
        bytes,
    )
}

/// Flattens with an explicit approximation criterion, retaining the same
/// subdivision order, metadata and limits as the canonical flattener.
pub(crate) fn flatten_with(
    path: &Path,
    transform: Transform,
    limits: &Limits,
    result: &mut Vec<Polyline>,
    pending: &mut CurveStack,
    flatness: Flatness,
    bytes: &mut Meter,
) -> Result<usize> {
    flatten_using(
        path,
        transform,
        limits,
        result,
        pending,
        Subdivision {
            flatness,
            bytes,
            inspection: None,
            source: Source::Fill,
        },
    )
}

/// The shared flattener with retained hull intervals and bounded inspection
/// work. Only quadratic glyph coverage certification needs this evidence.
pub(crate) fn flatten_inspected(
    path: &Path,
    transform: Transform,
    limits: &Limits,
    result: &mut Vec<Polyline>,
    pending: &mut CurveStack,
    subdivision: (Flatness, &mut Meter, Inspection<'_>),
) -> Result<usize> {
    let (flatness, bytes, inspection) = subdivision;
    flatten_using(
        path,
        transform,
        limits,
        result,
        pending,
        Subdivision {
            flatness,
            bytes,
            inspection: Some(inspection),
            source: Source::Fill,
        },
    )
}

fn flatten_using(
    path: &Path,
    transform: Transform,
    limits: &Limits,
    result: &mut Vec<Polyline>,
    pending: &mut CurveStack,
    mut subdivision: Subdivision<'_>,
) -> Result<usize> {
    let bytes = &mut *subdivision.bytes;
    pending.clear();
    validate(path, transform, limits)?;
    if let Some(inspection) = &mut subdivision.inspection {
        inspection.work.charge(path.segments().len())?;
    }
    let mut contour = 0;
    let mut count = 0;
    let mut current = prepare(result, contour, bytes)?;
    // A path opens every subpath with MoveTo, so the current point and the
    // subpath start are set before any line or curve reads them.
    let mut start = Point::default();
    let mut position = Point::default();
    for segment in path.segments() {
        // A line or curve following Close reopens at the subpath start.
        // Preserve the closed contour separately, including its point buffer.
        if current.closed
            && matches!(
                segment,
                Segment::LineTo(_) | Segment::CubicTo { .. } | Segment::QuadTo { .. }
            )
        {
            let first = current.points.first().copied();
            contour += 1;
            current = prepare(result, contour, bytes)?;
            if let Some(first) = first {
                bytes.reserve_capacity(&mut current.points, 1)?;
                current.points.push(first);
            }
        }
        let piece = match *segment {
            Segment::MoveTo(point) => {
                if !current.points.is_empty() {
                    contour += 1;
                    current = prepare(result, contour, bytes)?;
                }
                bounded_points([point], transform)?;
                append(&mut current.points, point, &mut count, limits, bytes)?;
                current.closed = false;
                current.degenerate = true;
                start = point;
                position = point;
                None
            }
            Segment::LineTo(point) => {
                bounded_points([point], transform)?;
                let joins = (subdivision.source != Source::Fill).then_some(&mut *bytes);
                current.record_tangents(position, [point; 3], false, joins)?;
                current.degenerate &= current.points.first().is_none_or(|p| *p == point);
                append(&mut current.points, point, &mut count, limits, bytes)?;
                position = point;
                None
            }
            Segment::CubicTo { c1, c2, end } => {
                bounded_points([c1, c2, end], transform)?;
                Some(([position, c1, c2, end], 0.0))
            }
            Segment::QuadTo { control, end } => {
                bounded_points([control, end], transform)?;
                let [c1, c2] = Segment::elevated(position, control, end);
                Some(([position, c1, c2, end], ELEVATION))
            }
            Segment::Close => {
                if subdivision.source != Source::Fill {
                    current.close_joins(bytes)?;
                }
                current.closed = true;
                position = start;
                None
            }
        };
        if let Some((controls @ [_, c1, c2, end], elevation)) = piece {
            let joins = (subdivision.source != Source::Fill).then_some(&mut *bytes);
            current.record_tangents(position, [c1, c2, end], true, joins)?;
            current.degenerate &= [c1, c2, end].iter().all(|p| *p == position);
            curve(
                (controls, elevation),
                transform,
                pending,
                (
                    &mut current.points,
                    (subdivision.source == Source::Dashed).then_some(&mut current.pieces),
                ),
                &mut count,
                limits,
                Subdivision {
                    flatness: subdivision.flatness,
                    source: Source::Fill,
                    bytes,
                    inspection: subdivision.inspection.as_mut().map(|i| Inspection {
                        hulls: i.hulls.as_deref_mut(),
                        work: i.work,
                        rounding: i.rounding,
                        targets: i.targets,
                        offset: i.offset,
                        factor: i.factor,
                    }),
                },
            )?;
            position = end;
        }
    }
    Ok(contour + usize::from(!current.points.is_empty()))
}

fn bounded_points<const N: usize>(points: [Point; N], transform: Transform) -> Result<()> {
    for point in points {
        bounded(point)?;
        bounded(point * transform)?;
    }
    Ok(())
}

fn prepare<'a>(
    result: &'a mut Vec<Polyline>,
    index: usize,
    bytes: &mut Meter,
) -> Result<&'a mut Polyline> {
    if index == result.len() {
        bytes.reserve_capacity(result, 1)?;
        result.push(Polyline {
            points: Vec::new(),
            closed: false,
            degenerate: true,
            tangents: [None; 2],
            curves: [false; 2],
            joins: Vec::new(),
            pieces: Vec::new(),
        });
    }
    let current = &mut result[index];
    current.points.clear();
    current.closed = false;
    current.degenerate = true;
    current.tangents = [None; 2];
    current.curves = [false; 2];
    current.joins.clear();
    current.pieces.clear();
    Ok(current)
}

fn append(
    points: &mut Vec<Point>,
    point: Point,
    count: &mut usize,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<()> {
    if *count >= limits.path_segments {
        return Err(Error::limit("flattened segment budget exceeded"));
    }
    if points.len() == points.capacity() {
        bytes.reserve_capacity(points, 1)?;
    }
    *count += 1;
    points.push(point);
    Ok(())
}

struct Subdivision<'a> {
    flatness: Flatness,
    bytes: &'a mut Meter,
    inspection: Option<Inspection<'a>>,
    source: Source,
}

/// Rounding units of the coordinates' magnitude that elevating a quadratic
/// adds to its cubic controls ([`Segment::elevated`]): its difference,
/// quotient and sum, mapped, with margin.
const ELEVATION: f64 = 16.0;

/// Flattens the cubic `points`, whose controls carry `elevation` rounding
/// units beyond the source's.
fn curve(
    (points, elevation): ([Point; 4], f64),
    transform: Transform,
    pending: &mut CurveStack,
    (out, mut pieces): (&mut Vec<Point>, Option<&mut Vec<CurvePiece>>),
    count: &mut usize,
    limits: &Limits,
    subdivision: Subdivision<'_>,
) -> Result<()> {
    let Subdivision {
        flatness,
        bytes,
        mut inspection,
        source: _,
    } = subdivision;
    let magnitude = points
        .iter()
        .map(|p| {
            ((p.x * transform.a).abs() + (p.y * transform.c).abs() + transform.e.abs())
                .max((p.x * transform.b).abs() + (p.y * transform.d).abs() + transform.f.abs())
        })
        .fold(1.0_f64, f64::max);
    let mapped = points.map(|p| p * transform);
    let near = Near::new(mapped, magnitude);
    let mut points = points;
    let mut depth = 0;
    // A piece shares its start with the piece before it and its end with
    // the piece split off after it, so each end is mapped once.
    let [mut start, .., mut end] = mapped;
    let finite_rectangle =
        |b: &[f64; 4]| b.iter().all(|v| v.is_finite()) && b[0] < b[2] && b[1] < b[3];
    let local_proof = inspection.as_ref().is_some_and(|i| {
        i.hulls.is_some() && !i.targets.is_empty() && i.targets.iter().all(finite_rectangle)
    });
    loop {
        if let Some(inspection) = &mut inspection {
            inspection.work.charge(1 + inspection.targets.len())?;
        }
        let [a, b, c, d] = [start, points[1] * transform, points[2] * transform, end];
        // A Bézier lies in its control hull (ISO 32000-2, 8.5.2.2).
        // The tolerance neighbourhood of a segment is convex, so admitting
        // both inner controls certifies the whole curve, including cusps.
        let rounding = hull_rounding(magnitude, depth, elevation)
            + inspection.as_ref().map_or(0.0, |i| i.rounding);
        let hull = inspection
            .as_ref()
            .map(|_| Hull::new([a, b, c, d], rounding));
        let mut outside_targets = false;
        let flatness = match (flatness, &inspection, hull) {
            (Flatness::Area(area), Some(i), Some(hull)) => {
                let [x0, y0, x1, y1] = hull.bounds;
                let shifted = Hull {
                    bounds: [
                        (x0 + i.offset.x).next_down(),
                        (y0 + i.offset.y).next_down(),
                        (x1 + i.offset.x).next_up(),
                        (y1 + i.offset.y).next_up(),
                    ],
                    area: hull.area,
                };
                let meets = i.targets.iter().any(|&b| shifted.intersects(b));
                outside_targets = local_proof && finite_rectangle(&shifted.bounds) && !meets;
                if meets {
                    Flatness::Area(area * i.factor)
                } else {
                    Flatness::Area(area)
                }
            }
            (flatness, _, _) => flatness,
        };
        if outside_targets || flatness.accepts([a, b, c, d], depth, rounding, &near) {
            if let (Some(i), Some(hull)) = (&mut inspection, hull)
                && let Some(hulls) = &mut i.hulls
            {
                bytes.reserve_capacity(hulls, 1)?;
                hulls.push(hull);
            }
            if let Some(pieces) = &mut pieces {
                bytes.reserve_capacity(pieces, 1)?;
                pieces.push(CurvePiece {
                    index: out.len() - 1,
                    controls: [points[1], points[2]],
                });
            }
            append(out, points[3], count, limits, bytes)?;
            let Some(next) = pending.pop() else {
                return Ok(());
            };
            start = end;
            (points, end, depth) = next;
        } else {
            if depth >= MAX_SUBDIVISION_DEPTH {
                return Err(Error::limit("curve subdivision depth exceeded"));
            }
            let [a, b, c, d] = points;
            let ab = midpoint(a, b);
            let bc = midpoint(b, c);
            let cd = midpoint(c, d);
            let abc = midpoint(ab, bc);
            let bcd = midpoint(bc, cd);
            let mid = midpoint(abc, bcd);
            if pending.len() == pending.capacity() {
                bytes.reserve_capacity(pending, 1)?;
            }
            depth += 1;
            pending.push(([mid, bcd, cd, d], end, depth));
            points = [a, ab, abc, mid];
            end = mid * transform;
        }
    }
}

/// The rounding allowance of a piece's control hull at subdivision `depth`,
/// for controls of coordinate `magnitude` that carry `elevation` units.
fn hull_rounding(magnitude: f64, depth: usize, elevation: f64) -> f64 {
    magnitude
        * f64::EPSILON
        * (16.0 * f64::from(u32::try_from(depth + 4).unwrap_or(u32::MAX)) + elevation)
}

/// For each curve of `path` in order, a box under `transform` containing
/// every hull its subdivision accepts. Midpoints of coordinates stay between
/// them, so every subdivided control lies within the curve's control box,
/// and its image within the box's mapped corners, up to three rounding
/// units of the magnitude for each of the two mappings. The hulls add their
/// rounding allowance, at most that of the deepest subdivision.
pub(crate) fn curve_boxes(
    path: &Path,
    transform: Transform,
) -> impl Iterator<Item = [f64; 4]> + '_ {
    let (mut start, mut position) = (Point::default(), Point::default());
    path.segments().iter().filter_map(move |segment| {
        let (controls, elevation) = match *segment {
            Segment::MoveTo(point) => {
                (start, position) = (point, point);
                return None;
            }
            Segment::LineTo(point) => {
                position = point;
                return None;
            }
            Segment::Close => {
                position = start;
                return None;
            }
            Segment::CubicTo { c1, c2, end } => ([position, c1, c2, end], 0.0),
            Segment::QuadTo { control, end } => {
                let [c1, c2] = Segment::elevated(position, control, end);
                ([position, c1, c2, end], ELEVATION)
            }
        };
        position = controls[3];
        let (mut lo, mut hi) = (controls[0], controls[0]);
        for p in &controls[1..] {
            lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        let corners = [lo, Point::new(hi.x, lo.y), hi, Point::new(lo.x, hi.y)];
        let magnitude = corners
            .iter()
            .map(|p| {
                ((p.x * transform.a).abs() + (p.y * transform.c).abs() + transform.e.abs())
                    .max((p.x * transform.b).abs() + (p.y * transform.d).abs() + transform.f.abs())
            })
            .fold(1.0_f64, f64::max);
        let margin = hull_rounding(magnitude, MAX_SUBDIVISION_DEPTH, elevation)
            + 8.0 * f64::EPSILON * magnitude;
        let mapped = corners.map(|p| p * transform);
        let (mut lo, mut hi) = (mapped[0], mapped[0]);
        for p in &mapped[1..] {
            lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        Some([
            (lo.x - margin).next_down(),
            (lo.y - margin).next_down(),
            (hi.x + margin).next_up(),
            (hi.y + margin).next_up(),
        ])
    })
}

#[expect(
    clippy::manual_midpoint,
    reason = "source controls and their convex combinations are bounded to 1e12"
)]
fn midpoint(a: Point, b: Point) -> Point {
    // Source controls are bounded to 1e12 before subdivision, and convex
    // combinations stay bounded. Their sum cannot overflow, so the general
    // midpoint's overflow handling is unnecessary here.
    Point::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5)
}

/// [`close`]'s decisions at [`TOLERANCE`] for the pieces of one curve, made
/// mostly without its divisions.
///
/// With `u` a control less the chord's start and `d` the chord, the
/// distance from the control to the chord's line is `|u × d| / |d|`: no more
/// than its distance to the chord, and equal to it when `u·d` lies in
/// `[0, |d|²]`. Comparing `(u × d)²` with `tolerance²·|d|²`, and [`close`]'s
/// own computation with its hypot guard, both err from the exact squared
/// distance of these differences by less than `7ε·tolerance·r +
/// 24ε·tolerance² + 6ε²r²`, where `r` is the sum of the components'
/// magnitudes and `ε` is [`f64::EPSILON`]. Beyond the slack
/// `256ε·tolerance·(r + tolerance) + 256ε²r²` from the threshold they
/// therefore decide alike, as they do for projections kept `4ε·|u|₁|d|₁`
/// inside that interval. Subdivision and mapping move a piece's controls
/// less than `200ε·magnitude` from the curve's mapped control box, so that
/// box, widened by `2⁻¹⁸·magnitude`, bounds `r` for every piece; a magnitude
/// of at least 1 keeps the margin normal. Shorter chords, whose squares
/// could leave the normal range, and anything nearer the threshold take
/// [`close`].
struct Near {
    /// `tolerance²` plus the slack: lines farther away fail.
    above: f64,
    /// `tolerance²` less the slack: nearer lines pass within the chord.
    below: f64,
    /// The projections' rounding allowance.
    margin: f64,
}

impl Near {
    const SLACK: f64 = 256.0 * f64::EPSILON;
    const SHORTEST: f64 = 1e-200;

    fn new(controls: [Point; 4], magnitude: f64) -> Self {
        let [x0, y0, x1, y1] = controls.iter().fold(
            [
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ],
            |[x0, y0, x1, y1], p| [x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y)],
        );
        // A bound of |u|₁ and |d|₁, widened far beyond the moved controls.
        let extent = (x1 - x0) + (y1 - y0) + magnitude * 2f64.powi(-18);
        let reach = 2.0 * extent;
        let slack = Self::SLACK * (TOLERANCE * (reach + TOLERANCE) + f64::EPSILON * reach * reach);
        let tolerance = TOLERANCE * TOLERANCE;
        Self {
            above: tolerance + slack,
            below: tolerance - slack,
            margin: 4.0 * f64::EPSILON * extent * extent,
        }
    }

    /// Whether both inner controls lie within the tolerance of the chord.
    fn flat(&self, controls: [Point; 4]) -> bool {
        let [a, b, c, d] = controls;
        let (dx, dy) = (d.x - a.x, d.y - a.y);
        let square = dx * dx + dy * dy;
        if square >= Self::SHORTEST {
            let (bx, by, cx, cy) = (b.x - a.x, b.y - a.y, c.x - a.x, c.y - a.y);
            let (cross_b, cross_c) = (bx * dy - by * dx, cx * dy - cy * dx);
            let (cross_b, cross_c) = (cross_b * cross_b, cross_c * cross_c);
            let (above, below) = (self.above * square, self.below * square);
            if cross_b > above || cross_c > above {
                return false;
            }
            let (dot_b, dot_c) = (bx * dx + by * dy, cx * dx + cy * dy);
            let end = square * (1.0 - 4.0 * f64::EPSILON) - self.margin;
            if cross_b < below
                && cross_c < below
                && dot_b > self.margin
                && dot_c > self.margin
                && dot_b < end
                && dot_c < end
            {
                return true;
            }
        }
        near_threshold(controls)
    }
}

/// [`Near::flat`]'s rare fallback, kept out of the subdivision loop.
#[cold]
#[inline(never)]
fn near_threshold([a, b, c, d]: [Point; 4]) -> bool {
    close(b, a, d, TOLERANCE) && close(c, a, d, TOLERANCE)
}

fn close(p: Point, a: Point, b: Point, bound: f64) -> bool {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let square = dx * dx + dy * dy;
    let t = if square == 0.0 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / square).clamp(0.0, 1.0)
    };
    let x = p.x - a.x - t * dx;
    let y = p.y - a.y - t * dy;
    let distance = x * x + y * y;
    let tolerance = bound * bound;
    // Preserve hypot's rounding decision at the boundary. Away from this
    // guard, rounding the two products and their sum cannot cross it; the
    // guard assumes hypot is accurate to within a few ulps.
    if (distance - tolerance).abs() <= 16.0 * f64::EPSILON * tolerance {
        x.hypot(y) <= bound
    } else {
        distance <= tolerance
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PathBuilder;
    use pagura_core::ErrorKind;

    #[test]
    fn hull_area_rejects_a_determinant_lost_to_cancellation() {
        // The two large products round to the same f64 although their
        // exact difference is -1, so the triangle's exact area is 1/2.
        let a = Point::new(0.0, 0.0);
        let b = Point::new(999_999_999_999.0, 999_999_999_998.0);
        let c = Point::new(999_999_999_998.0, 999_999_999_997.0);
        assert_eq!(b.x * c.y - b.y * c.x, 0.0);
        let near = Near::new([a, b, c, a], 1.0);
        assert!(!Flatness::Area(1.0 / 4096.0).accepts([a, b, c, a], 0, 0.0, &near));
    }

    #[test]
    fn local_curve_proofs_only_refine_hulls_that_meet_the_target() {
        let controls = [
            Point::new(0.0, 0.0),
            Point::new(0.0, 100.0),
            Point::new(100.0, 100.0),
            Point::new(100.0, 0.0),
        ];
        let away = [200.0, 200.0, 201.0, 201.0];
        let near = [40.0, 74.0, 60.0, 76.0];
        let invalid = [f64::NAN, 200.0, 201.0, 201.0];
        let reversed = [201.0, 200.0, 200.0, 201.0];
        let allowance = 1.0 / 4096.0;
        for (targets, retain_hulls, coarse) in [
            (std::slice::from_ref(&away), true, true),
            (std::slice::from_ref(&away), false, false),
            (std::slice::from_ref(&near), true, false),
            (std::slice::from_ref(&invalid), true, false),
            (std::slice::from_ref(&reversed), true, false),
            (&[][..], true, false),
        ] {
            let mut out = Vec::new();
            let mut pending = CurveStack::new();
            let mut hulls = Vec::new();
            let mut count = 0;
            let mut work = Meter::new(100_000, crate::band::RASTER_WORK);
            let mut bytes = crate::testing::bytes();
            curve(
                (controls, 0.0),
                Transform::IDENTITY,
                &mut pending,
                (&mut out, None),
                &mut count,
                &Limits::default(),
                Subdivision {
                    source: Source::Fill,
                    flatness: Flatness::Area(allowance),
                    bytes: &mut bytes,
                    inspection: Some(Inspection {
                        hulls: retain_hulls.then_some(&mut hulls),
                        work: &mut work,
                        rounding: 0.0,
                        targets,
                        offset: Point::default(),
                        factor: 1.0,
                    }),
                },
            )
            .unwrap();
            if coarse {
                assert_eq!(out, [controls[3]]);
                assert_eq!(hulls.len(), 1);
                assert!(!hulls[0].intersects(away));
                assert_eq!(work.used(), 2);
            } else {
                // Actual geometry keeps its whole-outline allowance even
                // outside targets; local proofs refine a curve that meets one.
                assert!(out.len() > 1);
                let error: f64 = hulls
                    .iter()
                    .filter(|hull| targets.iter().any(|&target| hull.intersects(target)))
                    .map(|hull| hull.area)
                    .sum();
                assert!(error <= allowance);
            }
        }
    }

    #[test]
    fn curve_boxes_contain_every_accepted_hull() {
        let p = Point::new;
        let curves = [
            Segment::QuadTo {
                control: p(3.0, 9.0),
                end: p(7.0, -2.0),
            },
            Segment::CubicTo {
                c1: p(-5.0, 8.0),
                c2: p(12.0, 8.0),
                end: p(1.0, 0.0),
            },
            Segment::QuadTo {
                control: p(1e3 + 3.0, 1e3 - 7.0),
                end: p(1e3 + 9.0, 1e3 + 1.0),
            },
        ];
        let transforms = [
            Transform::IDENTITY,
            Transform::new(0.6, 0.8, -0.8, 0.6, 0.0, 0.0),
            Transform::new(1e-3, 2e-4, -3e-4, 7e-4, 0.0, 0.0),
        ];
        for (i, &segment) in curves.iter().enumerate() {
            // Far from the origin, rounding bounds the hull areas from below.
            let (start, allowance) = if i == 2 {
                (p(1e3, 1e3), 1e-7)
            } else {
                (p(0.0, 0.0), 1e-9)
            };
            let mut path = PathBuilder::new();
            path.move_to(start);
            match segment {
                Segment::QuadTo { control, end } => path.quad_to(control, end).unwrap(),
                Segment::CubicTo { c1, c2, end } => path.cubic_to(c1, c2, end).unwrap(),
                _ => unreachable!(),
            };
            let path = path.finish();
            for transform in transforms {
                let mut boxes = curve_boxes(&path, transform);
                let bounds = boxes.next().unwrap();
                assert!(boxes.next().is_none());
                let mut hulls = Vec::new();
                let mut work = Meter::new(usize::MAX, crate::band::RASTER_WORK);
                flatten_inspected(
                    &path,
                    transform,
                    &Limits::default(),
                    &mut Vec::new(),
                    &mut CurveStack::new(),
                    (
                        Flatness::Area(allowance),
                        &mut crate::testing::bytes(),
                        Inspection {
                            hulls: Some(&mut hulls),
                            work: &mut work,
                            rounding: 0.0,
                            targets: &[],
                            offset: Point::default(),
                            factor: 1.0,
                        },
                    ),
                )
                .unwrap();
                assert!(hulls.len() > 1);
                for hull in hulls {
                    let [x0, y0, x1, y1] = hull.bounds;
                    assert!(bounds[0] <= x0 && bounds[1] <= y0, "{bounds:?} {hull:?}");
                    assert!(x1 <= bounds[2] && y1 <= bounds[3], "{bounds:?} {hull:?}");
                }
            }
        }
    }

    #[test]
    fn subdivision_stops_at_the_depth_bound() {
        let corner = [
            Point::new(0.0, 0.0),
            Point::new(0.0, 100.0),
            Point::new(100.0, 100.0),
            Point::new(100.0, 0.0),
        ];
        let (mut points, mut count, mut pending) = (Vec::new(), 0, CurveStack::new());
        let limits = Limits::default();
        let mut bytes = crate::testing::bytes();
        let mut flatten = |transform| {
            curve(
                (corner, 0.0),
                transform,
                &mut pending,
                (&mut points, None),
                &mut count,
                &limits,
                Subdivision {
                    source: Source::Fill,
                    flatness: Flatness::Distance,
                    bytes: &mut bytes,
                    inspection: None,
                },
            )
        };
        flatten(Transform::IDENTITY).unwrap();
        // Far beyond MAX_COORDINATE, the curve spans about 1e102 pixels and
        // would need some 170 halvings to reach the tolerance.
        let error = flatten(Transform::scale(1e100, 1e100)).unwrap_err();
        assert_eq!(
            (error.kind, error.message),
            (ErrorKind::Limit, "curve subdivision depth exceeded")
        );
        assert_eq!(points.last(), Some(&corner[3]));
        assert!(pending.len() <= MAX_SUBDIVISION_DEPTH);
    }

    #[test]
    fn coordinates_beyond_the_working_range_are_limits() {
        assert!(bounded(Point::new(1e12, -1e12)).is_ok());
        assert_eq!(
            bounded(Point::new(-1.5e12, 0.0)).unwrap_err().kind,
            ErrorKind::Limit
        );
        assert_eq!(
            bounded(Point::new(0.0, f64::INFINITY)).unwrap_err().kind,
            ErrorKind::InvalidArgument
        );
        let mut path = PathBuilder::new();
        path.move_to(Point::new(1.0, 1.0));
        let path = path.finish();
        let limits = Limits::default();
        // In range in user space, beyond it in device space.
        let scaled = flatten(
            &path,
            Transform::scale(2e12, 1.0),
            Source::Fill,
            &limits,
            &mut crate::testing::bytes(),
        )
        .unwrap_err();
        assert_eq!(scaled.kind, ErrorKind::Limit);
        let invalid = flatten(
            &path,
            Transform::translate(f64::NAN, 0.0),
            Source::Fill,
            &limits,
            &mut crate::testing::bytes(),
        )
        .unwrap_err();
        assert_eq!(invalid.kind, ErrorKind::InvalidArgument);
    }

    #[test]
    fn curves_after_close_start_at_the_subpath_start() {
        let mut path = PathBuilder::new();
        path.polygon([Point::new(1.0, 1.0), Point::new(5.0, 1.0)]);
        path.cubic_to(
            Point::new(1.0, 3.0),
            Point::new(3.0, 5.0),
            Point::new(5.0, 5.0),
        )
        .unwrap();
        let contours = flatten(
            &path.finish(),
            Transform::IDENTITY,
            Source::Fill,
            &Limits::default(),
            &mut crate::testing::bytes(),
        )
        .unwrap();
        assert_eq!(contours.len(), 2);
        assert!(contours[0].closed && !contours[1].closed);
        assert_eq!(contours[1].points.first(), Some(&Point::new(1.0, 1.0)));
        assert_eq!(contours[1].points.last(), Some(&Point::new(5.0, 5.0)));
        assert!(contours[1].points.len() > 2);
    }

    fn distance(p: Point, a: Point, b: Point) -> f64 {
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let square = dx * dx + dy * dy;
        if square == 0.0 {
            return (p.x - a.x).hypot(p.y - a.y);
        }
        let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / square).clamp(0.0, 1.0);
        (p.x - a.x - t * dx).hypot(p.y - a.y - t * dy)
    }

    fn reference(points: [Point; 4], transform: Transform, out: &mut Vec<Point>) {
        let midpoint = |a: Point, b: Point| Point::new(a.x.midpoint(b.x), a.y.midpoint(b.y));
        let [a, b, c, d] = points.map(|p| p * transform);
        if distance(b, a, d).max(distance(c, a, d)) <= TOLERANCE {
            out.push(points[3]);
        } else {
            let [a, b, c, d] = points;
            let ab = midpoint(a, b);
            let bc = midpoint(b, c);
            let cd = midpoint(c, d);
            let abc = midpoint(ab, bc);
            let bcd = midpoint(bc, cd);
            let mid = midpoint(abc, bcd);
            reference([a, ab, abc, mid], transform, out);
            reference([mid, bcd, cd, d], transform, out);
        }
    }

    fn random(state: &mut u32) -> f64 {
        *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(*state) / f64::from(u32::MAX) * 128.0 - 64.0
    }

    #[test]
    fn bounded_midpoints_match_general_midpoints_including_subnormals() {
        let values = [
            -1e12,
            -1.0,
            -f64::MIN_POSITIVE,
            -f64::from_bits(1),
            -0.0,
            0.0,
            f64::from_bits(1),
            f64::MIN_POSITIVE,
            1.0,
            1e12,
        ];
        for a in values {
            for b in values {
                let point = midpoint(Point::new(a, -a), Point::new(b, -b));
                assert_eq!(point.x.to_bits(), a.midpoint(b).to_bits());
                assert_eq!(point.y.to_bits(), (-a).midpoint(-b).to_bits());
            }
        }
    }

    #[test]
    fn subdivision_points_match_the_recursive_reference() {
        let mut state = 0x7942_a1b3;
        let mut pending = Vec::new();
        for _ in 0..2048 {
            let points =
                std::array::from_fn(|_| Point::new(random(&mut state), random(&mut state)));
            let transform = Transform::new(
                random(&mut state),
                random(&mut state),
                random(&mut state),
                random(&mut state),
                random(&mut state),
                random(&mut state),
            );
            let mut expected = Vec::new();
            reference(points, transform, &mut expected);
            let mut actual = Vec::new();
            curve(
                (points, 0.0),
                transform,
                &mut pending,
                (&mut actual, None),
                &mut 0,
                &Limits::default(),
                Subdivision {
                    source: Source::Fill,
                    flatness: Flatness::Distance,
                    bytes: &mut crate::testing::bytes(),
                    inspection: None,
                },
            )
            .unwrap();
            assert_eq!(actual, expected);
            assert_eq!(pending.len(), 0);
        }
        // Exercise the hypot guard on both sides of the exact threshold,
        // with vertical, horizontal and degenerate chords.
        for i in -64..=64 {
            let y = TOLERANCE * (1.0 + f64::from(i) * f64::EPSILON);
            for (a, b, p) in [
                (
                    Point::new(0.0, 0.0),
                    Point::new(1.0, 0.0),
                    Point::new(0.5, y),
                ),
                (
                    Point::new(0.0, 0.0),
                    Point::new(0.0, 1.0),
                    Point::new(y, 0.5),
                ),
                (
                    Point::new(0.0, 0.0),
                    Point::new(0.0, 0.0),
                    Point::new(0.0, y),
                ),
            ] {
                assert_eq!(close(p, a, b, TOLERANCE), distance(p, a, b) <= TOLERANCE);
            }
        }
    }

    #[test]
    fn chord_line_decisions_match_the_exact_test() {
        let exact =
            |[a, b, c, d]: [Point; 4]| close(b, a, d, TOLERANCE) && close(c, a, d, TOLERANCE);
        let mut state = 0x2c1b_3c6d;
        for scale in [1e-6, 1.0, 1e3, 1e6, 1e9, 1e12] {
            for _ in 0..1 << 15 {
                let a = Point::new(
                    random(&mut state) * scale / 64.0,
                    random(&mut state) * scale / 64.0,
                );
                let length = (random(&mut state) / 256.0 + 0.75) * scale.min(64.0);
                let angle = random(&mut state);
                let (dx, dy) = (angle.cos() * length, angle.sin() * length);
                let d = Point::new(a.x + dx, a.y + dy);
                // Controls beside or beyond the chord, at every relative
                // distance from the threshold down to a few units of roundoff.
                let control = |state: &mut u32| {
                    let along = random(state) / 32.0;
                    let relative = 1.0 + 2f64.powf(random(state) * 0.47 - 30.0);
                    let relative = if random(state) > 0.0 {
                        relative
                    } else {
                        relative.recip()
                    };
                    let off = TOLERANCE * relative.sqrt() * random(state).signum();
                    Point::new(
                        a.x + along * dx - off * dy / length,
                        a.y + along * dy + off * dx / length,
                    )
                };
                let controls = [a, control(&mut state), control(&mut state), d];
                let magnitude = controls
                    .iter()
                    .map(|p| p.x.abs().max(p.y.abs()))
                    .fold(1.0, f64::max);
                let near = Near::new(controls, magnitude);
                assert_eq!(near.flat(controls), exact(controls), "{controls:?}");
                let point = [a, a, a, a];
                assert_eq!(near.flat(point), exact(point));
            }
        }
    }

    fn certify(points: [Point; 4], transform: Transform) {
        let mut out = vec![points[0]];
        curve(
            (points, 0.0),
            transform,
            &mut Vec::new(),
            (&mut out, None),
            &mut 1,
            &Limits::default(),
            Subdivision {
                source: Source::Fill,
                flatness: Flatness::Distance,
                bytes: &mut crate::testing::bytes(),
                inspection: None,
            },
        )
        .unwrap();
        // Evaluate Bernstein polynomials independently of the subdivision.
        // Work relative to the first control to retain precision at 1e12.
        let origin = points[0];
        let [a, b, c, d] = points.map(|p| Point::new(p.x - origin.x, p.y - origin.y));
        let linear = Transform {
            e: 0.0,
            f: 0.0,
            ..transform
        };
        let out: Vec<_> = out
            .iter()
            .map(|p| Point::new(p.x - origin.x, p.y - origin.y) * linear)
            .collect();
        for i in 0..=2048 {
            let t = f64::from(i) / 2048.0;
            let s = 1.0 - t;
            let weights = [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t];
            let point = Point::new(
                [a, b, c, d].iter().zip(weights).map(|(p, w)| p.x * w).sum(),
                [a, b, c, d].iter().zip(weights).map(|(p, w)| p.y * w).sum(),
            ) * linear;
            let error = out
                .windows(2)
                .map(|p| distance(point, p[0], p[1]))
                .fold(f64::INFINITY, f64::min);
            assert!(error <= TOLERANCE, "error {error}, controls {points:?}");
        }
    }

    #[test]
    fn dense_cubic_oracle_covers_loops_cusps_and_numeric_extremes() {
        for controls in [
            [(0.0, 0.0), (64.0, 64.0), (-64.0, 64.0), (0.0, 0.0)],
            [(0.0, 0.0), (64.0, 0.0), (-64.0, 0.0), (0.0, 0.0)],
            [(0.0, 0.0), (64.0, 1e-12), (-64.0, -1e-12), (1e-12, 0.0)],
            [(0.0, 0.0), (1e-100, 1e-100), (-1e-100, 1e-100), (0.0, 0.0)],
            [
                (1e12 - 128.0, 1e12 - 128.0),
                (1e12, 1e12),
                (1e12 - 256.0, 1e12),
                (1e12 - 128.0, 1e12 - 128.0),
            ],
        ] {
            certify(controls.map(|(x, y)| Point::new(x, y)), Transform::IDENTITY);
        }
        // Large and tiny user-space controls with compensating device scales.
        certify(
            [
                Point::new(0.0, 0.0),
                Point::new(1e12, 1e12),
                Point::new(-1e12, 1e12),
                Point::new(0.0, 0.0),
            ],
            Transform::scale(1e-10, 1e-10),
        );
        certify(
            [
                Point::new(0.0, 0.0),
                Point::new(1e-100, 1e-100),
                Point::new(-1e-100, 1e-100),
                Point::new(0.0, 0.0),
            ],
            Transform::scale(1e102, 1e102),
        );
        let mut state = 0xf37a_2219;
        for _ in 0..128 {
            certify(
                std::array::from_fn(|_| Point::new(random(&mut state), random(&mut state))),
                Transform::new(1.0, 0.125, -0.25, 1.0, 7.0, 3.0),
            );
        }
    }

    #[test]
    fn geometric_point_classification_precedes_curve_approximation() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(4.0, 4.0));
        path.cubic_to(
            Point::new(4.001, 4.0),
            Point::new(4.0, 4.001),
            Point::new(4.0, 4.0),
        )
        .unwrap();
        path.close();
        let contours = flatten(
            &path.finish(),
            Transform::IDENTITY,
            Source::Fill,
            &Limits::default(),
            &mut crate::testing::bytes(),
        )
        .unwrap();
        assert_eq!(contours[0].points[0], contours[0].points[1]);
        assert!(!contours[0].degenerate);
    }
    #[test]
    fn a_byte_refusal_precedes_the_first_contour_allocation() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(0.0, 0.0));
        path.cubic_to(
            Point::new(0.0, 1e12),
            Point::new(1e12, 1e12),
            Point::new(1e12, 0.0),
        )
        .unwrap();
        let path = path.finish();
        let (mut contours, mut pending) = (Vec::new(), CurveStack::new());
        for allowance in [0, 64] {
            let mut bytes = Meter::new(allowance, crate::band::RENDER_BYTES);
            let error = flatten_into(
                &path,
                Transform::IDENTITY,
                &Limits::default(),
                &mut contours,
                &mut pending,
                &mut bytes,
            )
            .unwrap_err();
            assert_eq!(error.message, crate::band::RENDER_BYTES);
            let storage = retained_bytes(&contours) + stack_bytes(&pending);
            assert!(storage <= bytes.used() && bytes.used() <= allowance);
            if allowance == 0 {
                assert_eq!((contours.capacity(), pending.capacity()), (0, 0));
            }
        }
    }
}
