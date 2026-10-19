//! Bounded adaptive subdivision in device space.

use crate::{Path, Point, Segment, Transform};
use pagura_core::{Error, Limits, Result};

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

pub(crate) type CurveStack = Vec<([Point; 4], usize)>;

/// A flattened contour with source closure and geometric-point metadata.
#[derive(Debug, PartialEq)]
pub struct Polyline {
    pub(crate) points: Vec<Point>,
    pub(crate) closed: bool,
    pub(crate) degenerate: bool,
}

impl Polyline {
    pub(crate) fn strokeable(&self, cap: crate::LineCap) -> bool {
        if self.points.len() < 2 && !self.closed {
            return false;
        }
        self.points.windows(2).any(|p| p[0] != p[1]) || cap == crate::LineCap::Round
    }
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

pub(crate) fn flatten(path: &Path, transform: Transform, limits: &Limits) -> Result<Vec<Polyline>> {
    let mut result = Vec::new();
    let count = flatten_into(path, transform, limits, &mut result, &mut CurveStack::new())?;
    result.truncate(count);
    Ok(result)
}

pub(crate) fn flatten_into(
    path: &Path,
    transform: Transform,
    limits: &Limits,
    result: &mut Vec<Polyline>,
    pending: &mut CurveStack,
) -> Result<usize> {
    pending.clear();
    validate(path, transform, limits)?;
    let mut contour = 0;
    let mut count = 0;
    let mut current = prepare(result, contour);
    // A path opens every subpath with MoveTo, so the current point and the
    // subpath start are set before any line or curve reads them.
    let mut start = Point::default();
    let mut position = Point::default();
    for segment in path.segments() {
        // A line or curve following Close reopens at the subpath start.
        // Preserve the closed contour separately, including its point buffer.
        if current.closed && matches!(segment, Segment::LineTo(_) | Segment::CubicTo { .. }) {
            let first = current.points.first().copied();
            contour += 1;
            current = prepare(result, contour);
            current.points.extend(first);
        }
        match *segment {
            Segment::MoveTo(point) => {
                if !current.points.is_empty() {
                    contour += 1;
                    current = prepare(result, contour);
                }
                bounded(point)?;
                bounded(point * transform)?;
                append(&mut current.points, point, &mut count, limits)?;
                current.closed = false;
                current.degenerate = true;
                start = point;
                position = point;
            }
            Segment::LineTo(point) => {
                bounded(point)?;
                bounded(point * transform)?;
                current.degenerate &= current.points.first().is_none_or(|p| *p == point);
                append(&mut current.points, point, &mut count, limits)?;
                position = point;
            }
            Segment::CubicTo { c1, c2, end } => {
                current.degenerate &= [c1, c2, end].iter().all(|p| *p == position);
                for point in [c1, c2, end] {
                    bounded(point)?;
                    bounded(point * transform)?;
                }
                curve(
                    [position, c1, c2, end],
                    transform,
                    pending,
                    &mut current.points,
                    &mut count,
                    limits,
                )?;
                position = end;
            }
            Segment::Close => {
                current.closed = true;
                position = start;
            }
        }
    }
    Ok(contour + usize::from(!current.points.is_empty()))
}

fn prepare(result: &mut Vec<Polyline>, index: usize) -> &mut Polyline {
    if index == result.len() {
        result.push(Polyline {
            points: Vec::new(),
            closed: false,
            degenerate: true,
        });
    }
    let current = &mut result[index];
    current.points.clear();
    current.closed = false;
    current.degenerate = true;
    current
}

fn append(points: &mut Vec<Point>, point: Point, count: &mut usize, limits: &Limits) -> Result<()> {
    if *count >= limits.path_segments {
        return Err(Error::limit("flattened segment budget exceeded"));
    }
    *count += 1;
    points.push(point);
    Ok(())
}

fn curve(
    points: [Point; 4],
    transform: Transform,
    pending: &mut CurveStack,
    out: &mut Vec<Point>,
    count: &mut usize,
    limits: &Limits,
) -> Result<()> {
    let mut points = points;
    let mut depth = 0;
    loop {
        let [a, b, c, d] = points.map(|p| p * transform);
        // A Bézier lies in its control hull (ISO 32000-2, 8.5.2.2).
        // The tolerance neighbourhood of a segment is convex, so admitting
        // both inner controls certifies the whole curve, including cusps.
        if close(b, a, d) && close(c, a, d) {
            append(out, points[3], count, limits)?;
            let Some(next) = pending.pop() else {
                return Ok(());
            };
            (points, depth) = next;
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
            pending
                .try_reserve(1)
                .map_err(|_| Error::limit("curve subdivision allocation failed"))?;
            depth += 1;
            pending.push(([mid, bcd, cd, d], depth));
            points = [a, ab, abc, mid];
        }
    }
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

fn close(p: Point, a: Point, b: Point) -> bool {
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
    let tolerance = TOLERANCE * TOLERANCE;
    // Preserve hypot's rounding decision at the boundary. Away from this
    // guard, rounding the two products and their sum cannot cross it; the
    // guard assumes hypot is accurate to within a few ulps.
    if (distance - tolerance).abs() <= 16.0 * f64::EPSILON * tolerance {
        x.hypot(y) <= TOLERANCE
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
    fn subdivision_stops_at_the_depth_bound() {
        let corner = [
            Point::new(0.0, 0.0),
            Point::new(0.0, 100.0),
            Point::new(100.0, 100.0),
            Point::new(100.0, 0.0),
        ];
        let (mut points, mut count, mut pending) = (Vec::new(), 0, CurveStack::new());
        let limits = Limits::default();
        let mut flatten = |transform| {
            curve(
                corner,
                transform,
                &mut pending,
                &mut points,
                &mut count,
                &limits,
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
        let scaled = flatten(&path, Transform::scale(2e12, 1.0), &limits).unwrap_err();
        assert_eq!(scaled.kind, ErrorKind::Limit);
        let invalid = flatten(&path, Transform::translate(f64::NAN, 0.0), &limits).unwrap_err();
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
        let contours = flatten(&path.finish(), Transform::IDENTITY, &Limits::default()).unwrap();
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
                points,
                transform,
                &mut pending,
                &mut actual,
                &mut 0,
                &Limits::default(),
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
                assert_eq!(close(p, a, b), distance(p, a, b) <= TOLERANCE);
            }
        }
    }

    fn certify(points: [Point; 4], transform: Transform) {
        let mut out = vec![points[0]];
        curve(
            points,
            transform,
            &mut Vec::new(),
            &mut out,
            &mut 1,
            &Limits::default(),
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
        let contours = flatten(&path.finish(), Transform::IDENTITY, &Limits::default()).unwrap();
        assert_eq!(contours[0].points[0], contours[0].points[1]);
        assert!(!contours[0].degenerate);
    }
}
