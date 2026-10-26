//! Stroke outlines in user space (ISO 32000-2, 8.4.3 and 8.5.3).
//!
//! Positively oriented component polygons form a union under nonzero fill.
//! Flattening is measured after the CTM; constructing the pen before the CTM
//! preserves elliptical pens under nonuniform transforms.

use crate::band::{Mapped, Polygon, RASTER_WORK, RENDER_BYTES};
use crate::flatten::{CurvePiece, Join, Source, TOLERANCE, bounded, flatten};
use crate::{Path, PathBuilder, Point, Transform};
use pagura_core::{Error, Limits, Meter, Result};

mod convex;
mod tiling;

/// Line cap style (8.4.3.3). Exhaustive: Table 53 closes the set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LineCap {
    /// 0: square end at the endpoint.
    #[default]
    Butt,
    /// 1: semicircular end.
    Round,
    /// 2: square end projecting half the line width.
    ProjectingSquare,
}

/// Line join style (8.4.3.4). Exhaustive: Table 54 closes the set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LineJoin {
    /// 0: mitered, subject to the miter limit.
    #[default]
    Miter,
    /// 1: circular arc.
    Round,
    /// 2: bevelled.
    Bevel,
}

/// Stroke parameters in the path's own (user) space; the stroker applies
/// the transform, so non-uniform scaling distorts the pen as PDF requires.
///
/// A plain record built with functional record update from
/// [`Stroke::default`], so it is not `#[non_exhaustive]`. Its fields are
/// checked when a stroke is outlined, not when it is built.
#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    /// Line width; 0 means the thinnest line the device can render.
    pub width: f64,
    /// Cap style.
    pub cap: LineCap,
    /// Join style.
    pub join: LineJoin,
    /// Miter limit, at least 1.
    pub miter_limit: f64,
    /// Dash lengths; empty means solid.
    pub dash_array: Vec<f64>,
    /// Dash phase.
    pub dash_phase: f64,
}

impl Default for Stroke {
    /// PDF's initial graphics state values (Table 51).
    fn default() -> Self {
        Self {
            width: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 10.0,
            dash_array: Vec::new(),
            dash_phase: 0.0,
        }
    }
}

/// Converts a user-space stroke to a fill outline and its device transform.
///
/// # Errors
/// Returns invalid geometry or exhausted caller budgets.
pub fn stroke_outline(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
) -> Result<(Path, Transform)> {
    let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
    outline(path, transform, stroke, limits, &mut bytes)
}

/// Converts an automatically grid-fitted stroke to a fill outline (10.7.5).
///
/// # Errors
/// As [`stroke_outline`]; fitted principal pen diameters must fit the numeric budget.
pub fn adjusted_stroke_outline(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
) -> Result<(Path, Transform)> {
    let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
    crate::adjust::outline(path, transform, stroke, limits, &mut bytes)
}

/// Adds `amount` to a stroker's work counter, which may not pass `maximum`.
fn charge(work: &mut usize, amount: usize, maximum: usize) -> Result<()> {
    *work = work
        .checked_add(amount)
        .filter(|&n| n <= maximum)
        .ok_or(Error::limit(RASTER_WORK))?;
    Ok(())
}

/// A dash retains its tangent even when its length is zero (8.5.3.2).
pub(crate) struct Run {
    pub(crate) points: Vec<Point>,
    pub(crate) closed: bool,
    tangent: Point,
    /// Source endpoint directions when they differ from the flattened chord.
    caps: [Option<Point>; 2],
    joins: Vec<Join>,
}
impl Run {
    fn new(tangent: Point) -> Self {
        Self {
            points: Vec::new(),
            joins: Vec::new(),
            tangent,
            closed: false,
            caps: [None; 2],
        }
    }

    fn push(
        &mut self,
        point: Point,
        source: Option<Join>,
        direction: Option<Point>,
        bytes: &mut Meter,
    ) -> Result<()> {
        bytes.reserve_capacity(&mut self.points, 1)?;
        // A subdivision vertex is smooth. Retain its source normal too: a
        // chord-normal strip can protrude past a nearby curved dash cap.
        if let Some(mut join) = source.or_else(|| {
            direction.map(|direction| Join {
                index: 0,
                tangents: [direction; 2],
            })
        }) {
            bytes.reserve_capacity(&mut self.joins, 1)?;
            join.index = self.points.len();
            self.joins.push(join);
        }
        self.points.push(point);
        Ok(())
    }

    fn heap_bytes(&self) -> usize {
        self.points
            .capacity()
            .saturating_mul(size_of::<Point>())
            .saturating_add(self.joins.capacity().saturating_mul(size_of::<Join>()))
    }

    /// Keep sparse source indices aligned when repeated vertices disappear.
    fn normalize(&mut self) {
        let mut next = 0;
        let mut joins = self.joins.iter_mut().peekable();
        for index in 0..self.points.len() {
            let point = self.points[index];
            if next == 0 || self.points[next - 1] != point {
                self.points[next] = point;
                next += 1;
            }
            while joins.peek().is_some_and(|join| join.index == index) {
                if let Some(join) = joins.next() {
                    join.index = next - 1;
                }
            }
        }
        self.points.truncate(next);
        self.joins.dedup_by(|later, earlier| {
            if later.index != earlier.index {
                return false;
            }
            earlier.tangents[1] = later.tangents[1];
            true
        });
        if self.closed && next > 1 && self.points.first() == self.points.last() {
            self.points.pop();
            if self.joins.last().is_some_and(|join| join.index == next - 1)
                && let Some(mut closing) = self.joins.pop()
            {
                closing.index = 0;
                if let Some(first) = self.joins.first_mut().filter(|join| join.index == 0) {
                    first.tangents[0] = closing.tangents[0];
                } else {
                    self.joins.insert(0, closing);
                }
            }
        }
        if self.closed {
            self.caps = [None; 2];
        } else {
            if let Some(join) = source_join(&self.joins, 0) {
                self.caps[0] = Some(join.tangents[1]);
            }
            if let Some(join) = source_join(&self.joins, self.points.len().saturating_sub(1)) {
                self.caps[1] = Some(join.tangents[0]);
            }
        }
    }

    pub(crate) fn transformed(mut self, transform: Transform) -> Self {
        for point in &mut self.points {
            *point = *point * transform;
        }
        // Scale the linear part before normalization to avoid overflow even
        // when a large finite CTM maps a very small user-space segment.
        let scale = transform
            .a
            .abs()
            .max(transform.b.abs())
            .max(transform.c.abs())
            .max(transform.d.abs());
        if scale > 0.0 {
            for direction in std::iter::once(&mut self.tangent)
                .chain(self.caps.iter_mut().flatten())
                .chain(self.joins.iter_mut().flat_map(|join| &mut join.tangents))
            {
                let x = (transform.a / scale) * direction.x + (transform.c / scale) * direction.y;
                let y = (transform.b / scale) * direction.x + (transform.d / scale) * direction.y;
                let tangent = Point::new(x, y);
                let length = tangent.length();
                if length > 0.0 {
                    *direction = tangent / length;
                }
            }
        }
        self.normalize();
        self
    }
}

/// The circular pen in construction space and its flattening tolerance.
#[derive(Clone, Copy)]
pub(crate) struct Pen {
    pub(crate) radius: f64,
    pub(crate) tolerance: f64,
}

/// How far the pieces of a stroke outline are known to tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Tiling {
    /// Pieces that meet without overlapping: the outline is its own
    /// boundary.
    Proven,
    /// Pieces whose union is the stroke, meeting without overlapping at
    /// every joint, which may overlap where the centreline comes back near
    /// itself.
    Unproven,
    /// The components, which overlap inside every turn.
    Components,
}

/// Where a stroke's outline polygons go as the stroker makes them, in its
/// construction space: a path, or the device polygon of that path.
pub(crate) trait Outline {
    /// The segments of the path so far: each polygon's points and a close.
    fn len(&self) -> usize;

    /// Heap bytes of the output so far.
    fn heap_bytes(&self) -> usize;

    /// Appends the closed polygon through `points`, in reverse order when
    /// `reversed`; the points are bounded and the budget checked.
    ///
    /// # Errors
    ///
    /// Returns exhausted budgets, and geometry beyond the working range.
    fn add_polygon(
        &mut self,
        points: &[Point],
        reversed: bool,
        limits: &Limits,
        bytes: &mut Meter,
    ) -> Result<()>;
}

impl Outline for PathBuilder {
    fn len(&self) -> usize {
        Self::len(self)
    }

    fn heap_bytes(&self) -> usize {
        Self::heap_bytes(self)
    }

    fn add_polygon(
        &mut self,
        points: &[Point],
        reversed: bool,
        _: &Limits,
        bytes: &mut Meter,
    ) -> Result<()> {
        self.reserve(points.len() + 1, bytes)?;
        if reversed {
            self.polygon(points.iter().rev().copied());
        } else {
            self.polygon(points.iter().copied());
        }
        Ok(())
    }
}

pub(crate) fn outline(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<(Path, Transform)> {
    outline_with(
        path,
        transform,
        stroke,
        limits,
        (Tiling::Components, TOLERANCE),
        |_| PathBuilder::new(),
        bytes,
    )
    .map(|(mut path, transform, _)| (path.finish(), transform))
}

/// The outline as pieces where every run admits them at least as well as
/// `accept`, else as components, and how far its pieces tile; round caps
/// and joins are flattened within the device tolerance `tolerance`.
#[cfg(test)]
pub(crate) fn pieces(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
    (accept, tolerance): (Tiling, f64),
    bytes: &mut Meter,
) -> Result<(Path, Transform, Tiling)> {
    outline_with(
        path,
        transform,
        stroke,
        limits,
        (accept, tolerance),
        |_| PathBuilder::new(),
        bytes,
    )
    .map(|(mut path, transform, tiling)| (path.finish(), transform, tiling))
}

/// [`pieces`] as the device polygon of the outline: the polygon that
/// [`Polygon::from_lines`] makes of the outline under its transform, built
/// as the stroker makes each piece.
pub(crate) fn polygon_pieces(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
    (accept, tolerance): (Tiling, f64),
    bytes: &mut Meter,
) -> Result<(Polygon, Tiling)> {
    outline_with(
        path,
        transform,
        stroke,
        limits,
        (accept, tolerance),
        Mapped::new,
        bytes,
    )
    .map(|(mapped, _, tiling)| (mapped.finish(), tiling))
}

/// [`polygon_pieces`] of an automatically grid-fitted stroke (10.7.5): the
/// fitted runs of [`crate::adjust`] as pieces where they tile.
pub(crate) fn adjusted_polygon_pieces(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
    accept: Tiling,
    bytes: &mut Meter,
) -> Result<(Polygon, Tiling)> {
    let before = bytes.used();
    let result = crate::adjust::runs(path, transform, stroke, limits, bytes).and_then(
        |(runs, pen, pen_transform)| {
            assemble(
                runs,
                (
                    pen,
                    pen_transform,
                    conformal(pen_transform) && undashed_miter(stroke),
                ),
                stroke,
                limits,
                accept,
                Mapped::new,
                bytes,
            )
        },
    );
    let retained = result.as_ref().map_or(0, |(out, _, _)| out.heap_bytes());
    bytes.refund(bytes.used().saturating_sub(before.saturating_add(retained)));
    result.map(|(mapped, _, tiling)| (mapped.finish(), tiling))
}

/// Whether `transform` maps circles to circles.
fn conformal(transform: Transform) -> bool {
    (transform.a == transform.d && transform.b == -transform.c)
        || (transform.a == -transform.d && transform.b == transform.c)
}

/// Whether a closed run's convex boundaries may serve as its pieces: an
/// undashed miter stroke (under a conformal pen).
fn undashed_miter(stroke: &Stroke) -> bool {
    stroke.dash_array.is_empty() && stroke.join == LineJoin::Miter
}

fn outline_with<O: Outline>(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
    (accept, device_tolerance): (Tiling, f64),
    fresh: impl Fn(Transform) -> O,
    bytes: &mut Meter,
) -> Result<(O, Transform, Tiling)> {
    let before = bytes.used();
    let result = construct(
        path,
        transform,
        stroke,
        limits,
        (accept, device_tolerance),
        fresh,
        bytes,
    );
    let retained = result.as_ref().map_or(0, |(out, _, _)| out.heap_bytes());
    bytes.refund(bytes.used().saturating_sub(before.saturating_add(retained)));
    result
}

/// The outline in `fresh(t)`, `t` the transform from construction space
/// to device space, which it also returns.
fn construct<O: Outline>(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
    (accept, device_tolerance): (Tiling, f64),
    fresh: impl Fn(Transform) -> O,
    bytes: &mut Meter,
) -> Result<(O, Transform, Tiling)> {
    validate(stroke, limits)?;
    // Dash distances belong to user space even for a device-width hairline
    // (8.4.3.6). Flatten with a device-space error bound, split there, and only
    // then transform each run before constructing the hairline pen.
    // The centreline keeps the exact tolerance: near a cusp, chords of
    // another tolerance turn otherwise and move whole joins.
    let source = if stroke.dash_array.is_empty() {
        Source::Stroke
    } else {
        Source::Dashed
    };
    let contours = flatten(path, transform, source, limits, bytes)?;
    let hairline = stroke.width == 0.0;
    let pen_transform = if hairline {
        Transform::IDENTITY
    } else {
        transform
    };
    let width = if hairline { 1.0 } else { stroke.width };
    let radius = width * 0.5;
    let stretch = pen_transform.a.hypot(pen_transform.b) + pen_transform.c.hypot(pen_transform.d);
    let tolerance = if stretch == 0.0 {
        device_tolerance
    } else {
        device_tolerance / stretch
    };
    // A closed run's convex boundaries serve as its pieces only for undashed
    // miter strokes whose pen maps circles to circles.
    let boundaries_allowed = undashed_miter(stroke) && conformal(transform);
    let pen = Pen { radius, tolerance };
    let mut runs = Vec::new();
    let contour_bytes = contours
        .capacity()
        .saturating_mul(size_of::<crate::flatten::Polyline>());
    for contour in contours {
        if !contour.strokeable(stroke.cap) {
            let released = contour.heap_bytes();
            drop(contour);
            bytes.refund(released);
            continue;
        }
        let dashed = dash_contour(contour, transform, stroke, limits, bytes)?;
        let dashed_bytes = dashed.capacity().saturating_mul(size_of::<Run>());
        for run in dashed {
            let run = if hairline {
                run.transformed(transform)
            } else {
                run
            };
            bytes.reserve_capacity(&mut runs, 1)?;
            runs.push(run);
        }
        bytes.refund(dashed_bytes);
    }
    bytes.refund(contour_bytes);
    assemble(
        runs,
        (pen, pen_transform, boundaries_allowed),
        stroke,
        limits,
        accept,
        fresh,
        bytes,
    )
}

/// The outline of `runs`, stroked with `pen` in construction space, in
/// `fresh(t)` for `t` the transform to device space: pieces where every run
/// admits them at least as well as `accept`, else components, and how far
/// its pieces tile. Closed runs take their convex boundaries as pieces where
/// `boundaries` allows.
fn assemble<O: Outline>(
    mut runs: Vec<Run>,
    (pen, pen_transform, boundaries): (Pen, Transform, bool),
    stroke: &Stroke,
    limits: &Limits,
    accept: Tiling,
    fresh: impl Fn(Transform) -> O,
    bytes: &mut Meter,
) -> Result<(O, Transform, Tiling)> {
    let mut cleanup_work = 0;
    let mut closed = Vec::new();
    if boundaries {
        bytes.reserve_capacity(&mut closed, runs.len())?;
        for run in &mut runs {
            closed.push(closed_boundary(
                run,
                pen,
                stroke.miter_limit,
                limits,
                &mut cleanup_work,
                bytes,
            )?);
        }
    }
    // Pieces replace the components when every run admits them; they cover
    // the same set.
    if accept < Tiling::Components {
        let mut tiles = fresh(pen_transform);
        let mut chains = tiling::Chains::default();
        let mut tiling = if tiling::apart(runs.iter(), pen.radius, stroke, bytes)? {
            Tiling::Proven
        } else {
            Tiling::Unproven
        };
        for (i, run) in runs.iter().enumerate() {
            if tiling > accept {
                break;
            }
            let built = match closed.get(i) {
                Some(Some(contours)) => {
                    convex::append(&mut tiles, contours, limits, bytes)?;
                    Tiling::Proven
                }
                _ => {
                    if tiling::append(&mut tiles, run, (pen, stroke), &mut chains, limits, bytes)? {
                        Tiling::Proven
                    } else {
                        Tiling::Unproven
                    }
                }
            };
            tiling = tiling.max(built);
        }
        // The chains stay charged across the runs, and no longer.
        let released = chains.heap_bytes();
        drop(chains);
        bytes.refund(released);
        if tiling <= accept {
            return Ok((tiles, pen_transform, tiling));
        }
        let released = tiles.heap_bytes();
        drop(tiles);
        bytes.refund(released);
    }
    let mut components = fresh(pen_transform);
    for run in &runs {
        draw(&mut components, run, pen, stroke, limits, bytes)?;
    }
    Ok((components, pen_transform, Tiling::Components))
}

/// Clean the repeated closing vertex before attempting convex boundaries.
fn closed_boundary(
    run: &mut Run,
    pen: Pen,
    miter_limit: f64,
    limits: &Limits,
    work: &mut usize,
    bytes: &mut Meter,
) -> Result<Option<[Vec<Point>; 2]>> {
    if !run.closed || !run.joins.is_empty() || run.points.len() < convex::MIN_VERTICES {
        return Ok(None);
    }
    // The component stroker uses the same cleanup: duplicate endpoints
    // are not geometric turns.
    charge(work, run.points.len(), limits.raster_work)?;
    run.points.dedup();
    if run.points.first() == run.points.last() {
        run.points.pop();
    }
    convex::boundaries(
        &run.points,
        pen.radius,
        pen.tolerance,
        miter_limit,
        limits,
        bytes,
    )
}

pub(crate) fn validate(stroke: &Stroke, limits: &Limits) -> Result<()> {
    if !stroke.width.is_finite()
        || stroke.width < 0.0
        || !stroke.miter_limit.is_finite()
        || stroke.miter_limit < 1.0
        || !stroke.dash_phase.is_finite()
        || stroke.dash_array.iter().any(|&n| !n.is_finite() || n < 0.0)
    {
        return Err(Error::invalid_argument("invalid stroke parameters"));
    }
    if stroke.dash_array.len() > limits.path_segments {
        return Err(Error::limit("dash array budget exceeded"));
    }
    if !stroke.dash_array.is_empty() && stroke.dash_array.iter().all(|&n| n == 0.0) {
        return Err(Error::invalid_argument("dash array is all zero"));
    }
    Ok(())
}

/// Dash in user space, retaining true source directions at both original
/// endpoints and cuts inside curves (8.4.3.6).
pub(crate) fn dash_contour(
    contour: crate::flatten::Polyline,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<Vec<Run>> {
    let ends = [
        contour.points.first().copied(),
        contour.points.last().copied(),
    ];
    let stretch = transform.a.hypot(transform.b) + transform.c.hypot(transform.d);
    let accuracy = TOLERANCE / 64.0 / stretch.max(1.0);
    let mut runs = dash(
        contour.points,
        contour.joins,
        (contour.pieces, accuracy),
        contour.closed,
        stroke,
        limits,
        bytes,
    )?;
    if !contour.closed {
        for (index, end) in ends.into_iter().enumerate() {
            if !contour.curves[index] {
                continue;
            }
            let Some(tangent) = contour.tangents[index] else {
                continue;
            };
            let run = if index == 0 {
                runs.first_mut()
            } else {
                runs.last_mut()
            };
            if let Some(run) = run {
                let mut points = run.points.iter();
                let endpoint = if index == 0 {
                    points.next()
                } else {
                    points.next_back()
                };
                if endpoint.copied() != end {
                    continue;
                }
                let adjacent = if index == 0 {
                    points.find(|point| Some(*point) != endpoint)
                } else {
                    points.rfind(|point| Some(*point) != endpoint)
                };
                if let (Some(&endpoint), Some(&adjacent)) = (endpoint, adjacent) {
                    let chord = if index == 0 {
                        adjacent - endpoint
                    } else {
                        endpoint - adjacent
                    };
                    if tangent != chord / chord.length() {
                        run.caps[index] = Some(tangent);
                    }
                }
            }
        }
    }
    Ok(runs)
}

fn dash(
    mut points: Vec<Point>,
    mut joins: Vec<Join>,
    (pieces, accuracy): (Vec<CurvePiece>, f64),
    closed: bool,
    stroke: &Stroke,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<Vec<Run>> {
    let before = bytes.used().saturating_sub(
        points
            .capacity()
            .saturating_mul(size_of::<Point>())
            .saturating_add(joins.capacity().saturating_mul(size_of::<Join>()))
            .saturating_add(pieces.capacity().saturating_mul(size_of::<CurvePiece>())),
    );
    let result = (|| {
        if stroke.dash_array.is_empty() {
            let mut runs = Vec::new();
            bytes.reserve(&mut runs, 1)?;
            runs.push(Run {
                points,
                closed,
                tangent: Point::new(1.0, 0.0),
                caps: [None; 2],
                joins,
            });
            return Ok(runs);
        }
        if closed
            && points.first() != points.last()
            && let Some(&first) = points.first()
        {
            bytes.reserve_capacity(&mut points, 1)?;
            points.push(first);
            if let Some(mut join) = source_join(&joins, 0) {
                join.index = points.len() - 1;
                bytes.reserve_capacity(&mut joins, 1)?;
                joins.push(join);
            }
        }
        split_dashes(
            &points,
            &joins,
            (&pieces, accuracy),
            closed,
            stroke,
            limits,
            bytes,
        )
    })()
    .map(|mut runs| {
        for run in &mut runs {
            run.normalize();
        }
        runs
    });
    drop(pieces);
    let retained = result.as_ref().map_or(0, |runs| {
        runs.iter().fold(
            runs.capacity().saturating_mul(size_of::<Run>()),
            |n, run| n.saturating_add(run.heap_bytes()),
        )
    });
    bytes.refund(bytes.used().saturating_sub(before.saturating_add(retained)));
    result
}

fn split_dashes(
    points: &[Point],
    joins: &[Join],
    (pieces, accuracy): (&[CurvePiece], f64),
    closed: bool,
    stroke: &Stroke,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<Vec<Run>> {
    let (mut index, mut remaining) = dash_start(stroke)?;
    let cycle_count = stroke.dash_array.len() * 2;
    let initial_on = index % 2 == 0;
    let mut result = Vec::new();
    let mut run = Run::new(Point::new(1.0, 0.0));
    let mut work = Meter::new(limits.raster_work, RASTER_WORK);
    let mut tangent = Point::new(1.0, 0.0);
    // Each source piece contributes at most its share of the length error;
    // two further shares cover inversion at the current cut.
    let count = u32::try_from(pieces.len())
        .map_err(|_| Error::limit("dash curve count exceeds numeric range"))?;
    let precision = accuracy / (f64::from(count) + 2.0);
    if count > 0 && precision <= 0.0 {
        return Err(Error::limit("dash curve precision underflow"));
    }
    let mut pieces = pieces.iter().peekable();
    for (index_in_path, pair) in points.windows(2).enumerate() {
        let [a, b] = [pair[0], pair[1]];
        let controls = pieces
            .next_if(|piece| piece.index == index_in_path)
            .map(|piece| [a, piece.controls[0], piece.controls[1], b]);
        let distance = if let Some(controls) = controls {
            curve_length(controls, precision, 0, &mut work)?
        } else {
            (b - a).length()
        };
        if distance == 0.0 {
            if index % 2 == 0 {
                for (point, index) in [(a, index_in_path), (b, index_in_path + 1)] {
                    run.push(point, source_join(joins, index), None, bytes)?;
                }
            }
            continue;
        }
        if a != b {
            tangent = (b - a) / (b - a).length();
        }
        let mut offset = 0.0;
        while offset < distance || remaining == 0.0 {
            work.charge(1)?;
            if result.len() + run.points.len() > limits.path_segments {
                return Err(Error::limit("dashed segment budget exceeded"));
            }
            let step = remaining.min(distance - offset);
            let end = offset + step;
            if step > 0.0 && end == offset {
                return Err(Error::limit("dash lengths below numeric resolution"));
            }
            let mut sample = |d: f64| {
                controls.map_or_else(
                    || Ok((a.lerp(b, d / distance), None)),
                    |curve| {
                        curve_sample(curve, (d, distance), precision, &mut work)
                            .map(|(point, direction)| (point, Some(direction)))
                    },
                )
            };
            if index % 2 == 0 {
                if run.points.is_empty() {
                    let (point, direction) = sample(offset)?;
                    run.caps[0] = direction;
                    run.push(
                        point,
                        (offset == 0.0)
                            .then(|| source_join(joins, index_in_path))
                            .flatten(),
                        direction,
                        bytes,
                    )?;
                }
                let (point, direction) = sample(end)?;
                run.caps[1] = direction;
                tangent = direction.unwrap_or(tangent);
                run.push(
                    point,
                    (end == distance)
                        .then(|| source_join(joins, index_in_path + 1))
                        .flatten(),
                    direction,
                    bytes,
                )?;
            }
            offset = end;
            remaining -= step;
            if remaining == 0.0 {
                if index % 2 == 0 && !run.points.is_empty() {
                    bytes.reserve_capacity(&mut result, 1)?;
                    run.tangent = tangent;
                    result.push(std::mem::replace(&mut run, Run::new(tangent)));
                }
                index = (index + 1) % cycle_count;
                remaining = dash_length(stroke, index);
            }
        }
    }
    if !run.points.is_empty() {
        bytes.reserve_capacity(&mut result, 1)?;
        run.tangent = tangent;
        result.push(run);
    }
    join_seam(&mut result, points, closed && initial_on, bytes)?;
    Ok(result)
}

/// A cubic's chord and control polygon bound its arclength. Subdivision
/// halves the assigned uncertainty, so the summed estimates retain the
/// caller's absolute error bound. Only dashed curves pay this work.
fn curve_length(points: [Point; 4], precision: f64, depth: usize, work: &mut Meter) -> Result<f64> {
    work.charge(1)?;
    let lower = (points[3] - points[0]).length();
    let upper: f64 = points.windows(2).map(|p| (p[1] - p[0]).length()).sum();
    if upper - lower <= 2.0 * precision {
        return Ok(f64::midpoint(lower, upper));
    }
    if depth == 64 {
        return Err(Error::limit("dash curve length below numeric resolution"));
    }
    let [left, right] = curve_split(points, 0.5);
    Ok(curve_length(left, precision * 0.5, depth + 1, work)?
        + curve_length(right, precision * 0.5, depth + 1, work)?)
}

fn curve_split([a, b, c, d]: [Point; 4], t: f64) -> [[Point; 4]; 2] {
    let (ab, bc, cd) = (a.lerp(b, t), b.lerp(c, t), c.lerp(d, t));
    let (abc, bcd) = (ab.lerp(bc, t), bc.lerp(cd, t));
    let point = abc.lerp(bcd, t);
    [[a, ab, abc, point], [point, bcd, cd, d]]
}

/// Invert source arclength by bounded bisection. The summed error of the
/// at most 64 partial measurements is at most half a precision share;
/// the final control polygon bounds the remaining position uncertainty.
fn curve_sample(
    source: [Point; 4],
    (mut offset, length): (f64, f64),
    precision: f64,
    work: &mut Meter,
) -> Result<(Point, Point)> {
    let mut t = if offset == 0.0 { 0.0 } else { 1.0 };
    if offset > 0.0 && offset < length {
        let mut piece = source;
        let (mut lo, mut hi) = (0.0, 1.0);
        for depth in 0..64 {
            work.charge(1)?;
            let upper: f64 = piece.windows(2).map(|p| (p[1] - p[0]).length()).sum();
            t = f64::midpoint(lo, hi);
            if upper <= precision {
                break;
            }
            if depth == 63 {
                return Err(Error::limit("dash curve cut below numeric resolution"));
            }
            let [left, right] = curve_split(piece, 0.5);
            let distance = curve_length(left, precision / 128.0, 0, work)?;
            if offset < distance {
                piece = left;
                hi = t;
            } else {
                piece = right;
                lo = t;
                offset -= distance;
            }
        }
    }
    let [left, right] = curve_split(source, t);
    let point = left[3];
    let direction = [
        right[1] - left[2],
        right[2] - left[1],
        source[3] - source[0],
    ]
    .into_iter()
    .find(|d| d.length() > 0.0)
    .ok_or(Error::limit("dash curve tangent below numeric resolution"))?;
    Ok((point, direction / direction.length()))
}

fn dash_start(stroke: &Stroke) -> Result<(usize, f64)> {
    let cycle_count = stroke
        .dash_array
        .len()
        .checked_mul(2)
        .ok_or(Error::limit("dash cycle size overflow"))?;
    let length = 2.0 * stroke.dash_array.iter().sum::<f64>();
    if !length.is_finite() {
        return Err(Error::limit("dash cycle exceeds numeric range"));
    }
    // Twice the array sum restores both the length and on/off parity, including
    // odd arrays (8.4.3.6). Euclidean reduction handles large negative phases
    // without repeated increments. An empty array returned before this step.
    let phase = stroke.dash_phase.rem_euclid(length);
    let mut phase = if phase == length { 0.0 } else { phase };
    let mut index = 0;
    while phase > 0.0 && phase >= dash_length(stroke, index) {
        phase -= dash_length(stroke, index);
        index = (index + 1) % cycle_count;
    }
    Ok((index, dash_length(stroke, index) - phase))
}

fn source_join(joins: &[Join], index: usize) -> Option<Join> {
    joins
        .binary_search_by_key(&index, |join| join.index)
        .ok()
        .map(|index| joins[index])
}

fn join_seam(
    result: &mut Vec<Run>,
    points: &[Point],
    closed: bool,
    bytes: &mut Meter,
) -> Result<()> {
    // A dash crossing the closing vertex has a join, not a pair of caps.
    let seam = points.first();
    let starts = result.first().and_then(|run| run.points.first()) == seam;
    let ends = result.last().and_then(|run| run.points.last()) == seam;
    if closed && starts && ends && !result.is_empty() {
        if result.len() == 1 {
            if let Some(run) = result.first_mut() {
                run.closed = true;
            }
        } else if let Some(mut tail) = result.pop()
            && let Some(head) = result.first_mut()
        {
            let offset = tail.points.len().saturating_sub(1);
            bytes.reserve_capacity(&mut tail.points, head.points.len().saturating_sub(1))?;
            tail.points.extend(head.points.iter().skip(1).copied());
            bytes.reserve_capacity(&mut tail.joins, head.joins.len())?;
            tail.joins.extend(head.joins.iter().map(|join| Join {
                index: join.index.saturating_add(offset),
                ..*join
            }));
            tail.caps[1] = head.caps[1];
            let released = head.heap_bytes();
            *head = tail;
            bytes.refund(released);
        }
    }
    Ok(())
}

fn dash_length(stroke: &Stroke, index: usize) -> f64 {
    stroke
        .dash_array
        .get(index % stroke.dash_array.len())
        .copied()
        .unwrap_or(0.0)
}

pub(crate) fn draw(
    out: &mut impl Outline,
    run: &Run,
    pen: Pen,
    stroke: &Stroke,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<()> {
    let before = bytes.used().saturating_sub(out.heap_bytes());
    let result = (|| {
        let Pen { radius, .. } = pen;
        let closed = run.closed;
        let points = &run.points;
        if points.len() < 2 {
            if let Some(&point) = points.first() {
                cap(out, point, run.tangent, pen, stroke.cap, limits, bytes)?;
                cap(out, point, -run.tangent, pen, stroke.cap, limits, bytes)?;
            }
            return Ok(());
        }
        let first = points
            .first()
            .copied()
            .ok_or(Error::invalid_argument("empty stroke"))?;
        let last = points
            .last()
            .copied()
            .ok_or(Error::invalid_argument("empty stroke"))?;
        let mut directions = Vec::new();
        let segments = points.len() - usize::from(!closed);
        for (i, (&a, &b)) in points
            .iter()
            .zip(points.iter().skip(1).chain(closed.then_some(&first)))
            .enumerate()
        {
            let d = (b - a) / (b - a).length();
            let start = if let Some(join) = source_join(&run.joins, i) {
                join.tangents[1]
            } else if i == 0 && !closed {
                run.caps[0].unwrap_or(d)
            } else {
                d
            };
            let next = (i + 1) % points.len();
            let end = if let Some(join) = source_join(&run.joins, next) {
                join.tangents[0]
            } else if i + 1 == segments && !closed {
                run.caps[1].unwrap_or(d)
            } else {
                d
            };
            let [na, nb] = [start.perp() * radius, end.perp() * radius];
            polygon(out, &[a + na, b + nb, b - nb, a - na], limits, bytes)?;
            bytes.reserve_capacity(&mut directions, 1)?;
            directions.push(d);
        }
        for (i, &point) in points.iter().enumerate() {
            let before = if i == 0 {
                if closed { directions.last() } else { None }
            } else {
                directions.get(i - 1)
            };
            let after = directions.get(i);
            match (before, after) {
                (Some(&a), Some(&b)) => join(
                    out,
                    point,
                    source_join(&run.joins, i).map_or([a, b], |join| join.tangents),
                    pen,
                    stroke,
                    limits,
                    bytes,
                )?,
                (None, Some(&d)) => cap(
                    out,
                    first,
                    -run.caps[0].unwrap_or(d),
                    pen,
                    stroke.cap,
                    limits,
                    bytes,
                )?,
                (Some(&d), None) => cap(
                    out,
                    last,
                    run.caps[1].unwrap_or(d),
                    pen,
                    stroke.cap,
                    limits,
                    bytes,
                )?,
                _ => {}
            }
        }
        Ok(())
    })();
    bytes.refund(
        bytes
            .used()
            .saturating_sub(before.saturating_add(out.heap_bytes())),
    );
    result
}

fn join(
    out: &mut impl Outline,
    point: Point,
    tangents: [Point; 2],
    pen: Pen,
    stroke: &Stroke,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<()> {
    let before = bytes.used().saturating_sub(out.heap_bytes());
    let result = (|| {
        let mut points = Vec::new();
        bytes.reserve(&mut points, 1)?;
        points.push(point);
        if join_into(point, tangents, pen, stroke, limits, &mut points, bytes)?.is_none() {
            return Ok(());
        }
        polygon(out, &points, limits, bytes)
    })();
    bytes.refund(
        bytes
            .used()
            .saturating_sub(before.saturating_add(out.heap_bytes())),
    );
    result
}

/// Appends the join polygon's vertices at `point` other than the joint
/// itself to `points`: from the end corner of the segment before, on the
/// outer side, to the start corner of the one after. Returns whether they
/// lie on the left of the segments, or `None` where no join is drawn.
pub(crate) fn join_into(
    point: Point,
    [a, b]: [Point; 2],
    pen: Pen,
    stroke: &Stroke,
    limits: &Limits,
    points: &mut Vec<Point>,
    bytes: &mut Meter,
) -> Result<Option<bool>> {
    let Pen { radius, tolerance } = pen;
    let cross = a.cross(b);
    if cross == 0.0 && (stroke.join != LineJoin::Round || a.dot(b) >= 0.0) {
        return Ok(None);
    }
    let side = if cross > 0.0 { -radius } else { radius };
    let n1 = a.perp() * side;
    let n2 = b.perp() * side;
    if stroke.join == LineJoin::Round {
        // A turn within one subdivision step takes no point between the
        // normals. Then cos turn = a·b exceeds cos step = 2c² - 1, with
        // c = cos(step / 2) as below, by far more than the rounding of
        // either side and of the angle the arc would compute.
        let c = 1.0 - (tolerance / radius).min(1.0);
        if a.dot(b) >= 2.0 * c * c - 1.0 + 1e-9 {
            bytes.reserve_capacity(points, 2)?;
            points.extend([point + n1, point + n2]);
            return Ok(Some(cross <= 0.0));
        }
        let start = n1.y.atan2(n1.x);
        let end = n2.y.atan2(n2.x);
        let angle = if cross > 0.0 {
            (end - start).rem_euclid(std::f64::consts::TAU)
        } else {
            -(start - end).rem_euclid(std::f64::consts::TAU)
        };
        let step = 2.0 * (1.0 - (tolerance / radius).min(1.0)).acos();
        let count = (angle.abs() / step).ceil().max(1.0);
        if !count.is_finite() || count > limits.path_segments as f64 || count > f64::from(u32::MAX)
        {
            return Err(Error::limit("round join subdivision budget exceeded"));
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "integer count bounded by the u32-compatible path budget above"
        )]
        let count = count as u32;
        bytes.reserve_capacity(points, count as usize + 1)?;
        points.push(point + n1);
        for i in 1..count {
            let angle = start + angle * f64::from(i) / f64::from(count);
            points.push(Point::new(
                point.x + radius * angle.cos(),
                point.y + radius * angle.sin(),
            ));
        }
        points.push(point + n2);
        return Ok(Some(cross <= 0.0));
    }
    bytes.reserve_capacity(points, 3)?;
    points.push(point + n1);
    let denominator = 1.0 + a.dot(b);
    if stroke.join == LineJoin::Miter && denominator > 0.0 {
        let m = (n1 + n2) / denominator;
        if m.length() <= radius * stroke.miter_limit {
            points.push(point + m);
        }
    }
    points.push(point + n2);
    Ok(Some(cross <= 0.0))
}

fn cap(
    out: &mut impl Outline,
    point: Point,
    direction: Point,
    pen: Pen,
    cap: LineCap,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<()> {
    let Pen { radius, .. } = pen;
    match cap {
        LineCap::Butt => Ok(()),
        LineCap::Round => circle(out, point, pen, limits, bytes),
        LineCap::ProjectingSquare => {
            let n = direction.perp() * radius;
            let end = point + direction * radius;
            polygon(
                out,
                &[point + n, end + n, end - n, point - n],
                limits,
                bytes,
            )
        }
    }
}

fn circle(
    out: &mut impl Outline,
    center: Point,
    pen: Pen,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<()> {
    let before = bytes.used().saturating_sub(out.heap_bytes());
    let result = (|| {
        if pen.radius == 0.0 {
            return Ok(());
        }
        let points = ring(center, pen, limits, bytes)?;
        polygon(out, &points, limits, bytes)
    })();
    bytes.refund(
        bytes
            .used()
            .saturating_sub(before.saturating_add(out.heap_bytes())),
    );
    result
}

/// The pen's canonical circle polygon around `center`, counterclockwise
/// from angle zero at equal angles, within the pen's tolerance; charged to
/// `bytes`.
fn ring(center: Point, pen: Pen, limits: &Limits, bytes: &mut Meter) -> Result<Vec<Point>> {
    let mut points = offsets(pen, limits, bytes)?;
    for point in &mut points {
        *point = center + *point;
    }
    Ok(points)
}

/// The vertices of the pen's canonical circle polygon less its centre: each
/// vertex of [`ring`] is its centre plus one of these, and its count.
fn offsets(pen: Pen, limits: &Limits, bytes: &mut Meter) -> Result<Vec<Point>> {
    let Pen { radius, tolerance } = pen;
    let angle = 2.0 * (1.0 - (tolerance / radius).min(1.0)).acos();
    let count = (std::f64::consts::TAU / angle).ceil().max(8.0);
    if !count.is_finite() || count > limits.path_segments as f64 || count > f64::from(u32::MAX) {
        return Err(Error::limit("round stroke subdivision budget exceeded"));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "integer count is bounded by the u32-compatible path budget above"
    )]
    let count = count as u32;
    let mut points = Vec::new();
    bytes.reserve(&mut points, count as usize)?;
    points.extend((0..count).map(|i| {
        let angle = std::f64::consts::TAU * f64::from(i) / f64::from(count);
        Point::new(radius * angle.cos(), radius * angle.sin())
    }));
    Ok(points)
}

fn polygon(
    out: &mut impl Outline,
    points: &[Point],
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<()> {
    let Some(&first) = points.first() else {
        return Ok(());
    };
    let area: f64 = points
        .iter()
        .zip(points.iter().skip(1).chain(std::iter::once(&first)))
        .map(|(&a, &b)| (a - first).cross(b - first))
        .sum();
    // Every component polygon is positively oriented (module comment).
    oriented(out, points, area < 0.0, limits, bytes)
}

/// Appends the closed polygon through `points`, reversed when `reversed`,
/// within the outline budget and the coordinate range.
fn oriented(
    out: &mut impl Outline,
    points: &[Point],
    reversed: bool,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<()> {
    if out
        .len()
        .checked_add(points.len() + 1)
        .is_none_or(|n| n > limits.path_segments)
    {
        return Err(Error::limit("stroke outline budget exceeded"));
    }
    if points.is_empty() {
        return Ok(());
    }
    for &point in points {
        bounded(point)?;
    }
    out.add_polygon(points, reversed, limits, bytes)
}

#[cfg(test)]
mod tests;
