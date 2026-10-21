//! Empty filling regions retain their boundary during scan conversion (10.7.4).
//!
//! A closed polygon has empty interior exactly when its directed edges cancel
//! on every supporting line (modulo two for even-odd). This avoids a signed-area
//! heuristic: a bow tie has zero signed area but a nonempty filling region.
//! Edges are grouped by a rounded line key, then confirmed exactly, so rounding
//! can only keep ordinary filling. An oblique segment retraced through its
//! midpoint can round to different keys; it then fills ordinarily and paints
//! nothing.

use crate::flatten::{bounded, flatten};
use crate::{FillRegion, FillRule, Path, PathBuilder, Point, Segment, Stroke, Transform};
use pagura_core::{Error, Limits, Result};

/// Classify in construction space once; affine device placement must not
/// collapse a positive-area sliver into a certified empty region.
#[inline]
pub(crate) fn classify(
    path: &Path,
    rule: FillRule,
    limits: &Limits,
    work: &mut usize,
) -> Result<FillRegion> {
    if path.segments().len() > limits.path_segments {
        return Err(Error::limit("path segment budget exceeded"));
    }
    charge(work, path.segments().len(), limits)?;
    if ordinary_rectangle(path)? || ordinary_polygon(path)? {
        return Ok(FillRegion::Ordinary);
    }
    classify_uncertain(path, rule, limits, work)
}

// Keep allocation and sorting out of the ordinary fill's stack frame.
fn classify_uncertain(
    path: &Path,
    rule: FillRule,
    limits: &Limits,
    work: &mut usize,
) -> Result<FillRegion> {
    // Uncertain paths need a second source traversal before flattening.
    charge(work, path.segments().len(), limits)?;
    if !linear_curves(path)? {
        return Ok(FillRegion::Ordinary);
    }
    let contours = flatten(path, Transform::IDENTITY, limits)?;
    let mut edges = Vec::new();
    for contour in &contours {
        let Some(&first) = contour.points.first() else {
            continue;
        };
        let mut previous = first;
        for &next in contour.points.iter().skip(1).chain(std::iter::once(&first)) {
            charge(work, 1, limits)?;
            if previous != next {
                edges.push(Edge::new(previous, next));
            }
            previous = next;
        }
    }
    if edges.is_empty() {
        return Ok(if contours.iter().all(|contour| contour.degenerate) {
            FillRegion::Points
        } else {
            FillRegion::Ordinary
        });
    }
    charge(work, sort_work(edges.len()), limits)?;
    edges.sort_unstable_by(|a, b| {
        a.key[0]
            .total_cmp(&b.key[0])
            .then(a.key[1].total_cmp(&b.key[1]))
            .then(a.key[2].total_cmp(&b.key[2]))
    });
    let mut first = 0;
    while first < edges.len() {
        let edge = &edges[first];
        let mut end = first + 1;
        while end < edges.len() && edges[end].key == edge.key {
            end += 1;
        }
        let mut line = Line::new(edge.a, edge.b);
        charge(work, (end - first).saturating_mul(2), limits)?;
        for edge in &edges[first..end] {
            // A rounded-key collision or split can only reject certification.
            // Never search other groups, even when the geometry is uncertain.
            if !line.contains(edge.a, edge.b) {
                return Ok(FillRegion::Ordinary);
            }
            line.add(edge.a, edge.b);
        }
        charge(work, sort_work(line.events.len()), limits)?;
        if !line.cancels(rule)? {
            return Ok(FillRegion::Ordinary);
        }
        first = end;
    }
    Ok(FillRegion::EmptyBoundary)
}

fn sort_work(count: usize) -> usize {
    count.saturating_mul(count.max(1).ilog2() as usize + 1)
}

struct Edge {
    a: Point,
    b: Point,
    key: [f64; 3],
}
impl Edge {
    fn new(a: Point, b: Point) -> Self {
        // Compute the approximate key in one direction, even for retraced
        // edges: reversing the subtraction/intercept can round differently.
        let (first, last) = if a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)).is_le() {
            (a, b)
        } else {
            (b, a)
        };
        let dx = last.x - first.x;
        let dy = last.y - first.y;
        let key = if dx.abs() >= dy.abs() {
            let slope = dy / dx;
            [0.0, slope + 0.0, (first.y - slope * first.x) + 0.0]
        } else {
            let slope = dx / dy;
            [1.0, slope + 0.0, (first.x - slope * first.y) + 0.0]
        };
        Self { a, b, key }
    }
}

/// Allocation-free sufficient test for BOTH winding rules; no flattening.
///
/// A unique lexicographic maximum among the vertices of all subpaths is exposed
/// by some supporting direction (x + epsilon*y, with epsilon sufficiently
/// small). Nonincident edges, in its own subpath or any other, are strictly
/// below its supporting line. Thus, in a small neighbourhood of this vertex,
/// only its two incident edges occur. If their directions differ, the wedge
/// between them has winding +/-1, hence also odd parity. This works for
/// self-intersections and zero signed area, and cannot accept a twice-traced
/// contour. Curves conservatively fall through. The caller charges the entire
/// source traversal before this test.
fn ordinary_polygon(path: &Path) -> Result<bool> {
    let segments = path.segments();
    let vertex = |i| match segments[i] {
        Segment::MoveTo(p) | Segment::LineTo(p) => p,
        _ => unreachable!("subpath vertices are moves and lines"),
    };
    let mut maximum: Option<(Point, usize)> = None;
    let mut unique = false;
    let mut contour = 0..0;
    let mut start = 0;
    while start < segments.len() {
        let Segment::MoveTo(first) = segments[start] else {
            return Ok(false);
        };
        let mut end = start + 1;
        while let Some(Segment::LineTo(_)) = segments.get(end) {
            end += 1;
        }
        // Repeated closes only add zero-length edges at the subpath start.
        let mut next = end;
        while segments.get(next) == Some(&Segment::Close) {
            next += 1;
        }
        // An explicit closing line is the same edge as implicit closure, not a
        // second visit to the first vertex. Keep interior repetitions significant.
        if end > start + 1 && segments[end - 1] == Segment::LineTo(first) {
            end -= 1;
        }
        for i in start..end {
            let point = vertex(i);
            bounded(point)?;
            match maximum {
                Some((m, _)) if point.x < m.x || (point.x == m.x && point.y <= m.y) => {
                    unique &= point != m;
                }
                _ => {
                    maximum = Some((point, i));
                    unique = true;
                }
            }
        }
        if maximum.is_some_and(|(_, i)| i >= start) {
            contour = start..end;
        }
        start = next;
    }
    let Some((maximum, index)) = maximum else {
        return Ok(false);
    };
    if !unique || contour.len() < 3 {
        return Ok(false);
    }
    let previous = vertex(if index == contour.start {
        contour.end - 1
    } else {
        index - 1
    });
    let next = vertex(if index + 1 == contour.end {
        contour.start
    } else {
        index + 1
    });
    Ok(noncollinear(previous, maximum, next))
}

// Keep the common re check inline and independent of the polygon traversal.
// Opposite corners bound every coordinate of this axis-aligned rectangle.
#[inline]
fn ordinary_rectangle(path: &Path) -> Result<bool> {
    if let [
        Segment::MoveTo(a),
        Segment::LineTo(b),
        Segment::LineTo(c),
        Segment::LineTo(d),
        Segment::Close,
    ] = path.segments()
        && a.y == b.y
        && b.x == c.x
        && c.y == d.y
        && d.x == a.x
        && a.x != b.x
        && a.y != d.y
    {
        bounded(*a)?;
        bounded(*c)?;
        return Ok(true);
    }
    Ok(false)
}

fn noncollinear(a: Point, b: Point, c: Point) -> bool {
    let left = (a.x - b.x) * (c.y - b.y);
    let right = (a.y - b.y) * (c.x - b.x);
    // Each product contains two subtractions and a multiplication. Including
    // the final subtraction, a relative bound of 16*EPSILON on their absolute
    // sum exceeds gamma(8). Coordinates are bounded by 1e12, so products cannot
    // overflow. The absolute term also dominates gradual-underflow errors in
    // the differences/products (including multiplication by a 2e12 difference).
    let error = (left.abs() + right.abs()) * (16.0 * f64::EPSILON) + f64::MIN_POSITIVE * 1e14;
    (left - right).abs() > error
}

/// Converts a retained certificate into device hairlines. Ordinary raster paths
/// (including all stroke outlines) do no classification here.
#[inline]
pub(crate) fn outline(
    path: &Path,
    transform: Transform,
    rule: FillRule,
    limits: &Limits,
) -> Result<Option<Path>> {
    if path.fill_region(rule) != Some(FillRegion::EmptyBoundary) {
        return Ok(None);
    }
    outline_boundary(path, transform, rule, limits).map(Some)
}

pub(crate) fn outline_boundary(
    path: &Path,
    transform: Transform,
    rule: FillRule,
    limits: &Limits,
) -> Result<Path> {
    if !transform.is_finite() {
        return Err(Error::invalid_argument("path transform is not finite"));
    }
    let contours = flatten(path, transform, limits)?;
    let mut boundary = PathBuilder::new();
    let mut points = Vec::new();
    for (i, contour) in contours.iter().enumerate() {
        if i + 1 == contours.len() && !contour.closed && contour.points.len() == 1 {
            continue;
        }
        let Some(&first) = contour.points.first() else {
            continue;
        };
        // Use the original point classification, including its existing
        // distinction from tiny curves and repeated flattened vertices.
        if contour.degenerate {
            let point = first * transform;
            // Canonicalize signed zero so equal floor cells stay adjacent.
            points.push(Point::new(point.x.floor() + 0.0, point.y.floor() + 0.0));
            continue;
        }
        boundary.polygon(contour.points.iter().map(|&point| point * transform));
    }
    let (outline, _) = crate::stroke::outline(
        &boundary.finish(),
        Transform::IDENTITY,
        &Stroke {
            width: 0.0,
            ..Stroke::default()
        },
        limits,
    )?;
    // The stroker emits positively oriented polygons, so these cells and the
    // hairline boundaries form one union, painted exactly once.
    let mut result = PathBuilder::new();
    result.append(&outline, Transform::IDENTITY);
    points.sort_unstable_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    let mut points = points.iter().peekable();
    while let Some(&point) = points.next() {
        let mut odd = true;
        while points.next_if(|&&next| next == point).is_some() {
            odd = !odd;
        }
        if rule == FillRule::NonZero || odd {
            result.rect(point.x, point.y, 1.0, 1.0);
        }
    }
    if result.len() > limits.path_segments {
        return Err(Error::limit("empty fill outline segment budget exceeded"));
    }
    Ok(result.finish())
}

fn linear_curves(path: &Path) -> Result<bool> {
    let mut current = Point::default();
    let mut start = current;
    for segment in path.segments() {
        match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => {
                bounded(p)?;
                current = p;
                if matches!(segment, Segment::MoveTo(_)) {
                    start = current;
                }
            }
            Segment::CubicTo { c1: a, c2: b, end } => {
                for p in [a, b, end] {
                    bounded(p)?;
                }
                let controls = [current, a, b, end];
                if let Some(&other) = controls.iter().find(|&&p| p != current)
                    && !controls.iter().all(|&p| collinear(current, other, p))
                {
                    return Ok(false);
                }
                current = end;
            }
            Segment::Close => current = start,
        }
    }
    Ok(true)
}

struct Line {
    a: Point,
    b: Point,
    vertical: bool,
    events: Vec<(f64, i64)>,
}
impl Line {
    fn new(a: Point, b: Point) -> Self {
        Self {
            a,
            b,
            vertical: a.x == b.x,
            events: Vec::new(),
        }
    }
    fn contains(&self, a: Point, b: Point) -> bool {
        collinear(self.a, self.b, a) && collinear(self.a, self.b, b)
    }
    fn add(&mut self, a: Point, b: Point) {
        let (a, b) = if self.vertical {
            (a.y, b.y)
        } else {
            (a.x, b.x)
        };
        let direction = if a < b { 1 } else { -1 };
        self.events.push((a.min(b), direction));
        self.events.push((a.max(b), -direction));
    }
    fn cancels(&mut self, rule: FillRule) -> Result<bool> {
        self.events.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let mut winding = 0_i64;
        let mut events = self.events.iter().peekable();
        while let Some(&(at, delta)) = events.next() {
            winding = winding
                .checked_add(delta)
                .ok_or(Error::limit("winding number overflow"))?;
            while let Some(&&(next, delta)) = events.peek()
                && next == at
            {
                winding = winding
                    .checked_add(delta)
                    .ok_or(Error::limit("winding number overflow"))?;
                events.next();
            }
            if events.peek().is_some()
                && match rule {
                    FillRule::NonZero => winding != 0,
                    FillRule::EvenOdd => winding % 2 != 0,
                }
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

// Certify collinearity, rather than treating a small determinant as zero.
// Error-free subtraction and fused product residuals distinguish lines from
// arbitrarily thin triangles. Uncertain extreme cases keep ordinary filling.
fn collinear(a: Point, b: Point, p: Point) -> bool {
    if a == p || b == p || (a.x == b.x && p.x == a.x) || (a.y == b.y && p.y == a.y) {
        return true;
    }
    let Some([dx, dy, px, py]) = difference(b.x, a.x)
        .zip(difference(b.y, a.y))
        .zip(difference(p.x, a.x))
        .zip(difference(p.y, a.y))
        .map(|(((dx, dy), px), py)| [dx, dy, px, py])
    else {
        return false;
    };
    let (left, right) = (dx * py, dy * px);
    let minimum = f64::MIN_POSITIVE / (f64::EPSILON * f64::EPSILON);
    left.abs() >= minimum && left == right && dx.mul_add(py, -left) == dy.mul_add(px, -right)
}

fn difference(a: f64, b: f64) -> Option<f64> {
    let value = a - b;
    let back = a - value;
    let error = (a - (value + back)) + (back - b);
    (error == 0.0).then_some(value)
}

fn charge(work: &mut usize, amount: usize, limits: &Limits) -> Result<()> {
    *work = work
        .checked_add(amount)
        .filter(|&n| n <= limits.raster_work)
        .ok_or(Error::limit("empty fill boundary work budget exceeded"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Mask, Paint, Pixmap, Rgba8, fill_path, fill_path_at};

    #[test]
    fn exact_collinearity_does_not_widen_a_nearly_collinear_triangle() {
        let a = Point::new(0.0, 0.0);
        let b = Point::new(1.0 + f64::EPSILON, 1.0);
        let p = Point::new(1.0, 1.0 - f64::EPSILON);
        // The rounded determinant is zero; the exact determinant is -eps^2.
        assert_eq!(b.x * p.y, b.y * p.x);
        assert!(!collinear(a, b, p));
        assert!(collinear(a, Point::new(3.0, 4.0), Point::new(1.5, 2.0)));
        let mut builder = PathBuilder::new();
        builder
            .move_to(a)
            .line_to(b)
            .unwrap()
            .line_to(p)
            .unwrap()
            .close();
        let mut path = builder.finish();
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            path.classify_fill(rule, &Limits::default(), &mut 0)
                .unwrap();
            assert_eq!(path.fill_region(rule), Some(FillRegion::Ordinary));
        }
    }

    #[test]
    fn placement_and_linear_rounding_never_widen_positive_area() {
        let mut path = PathBuilder::new();
        path.rect(0.0, 0.0, 1e-9, 4.0);
        let mut path = path.finish();
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            path.classify_fill(rule, &Limits::default(), &mut 0)
                .unwrap();
        }
        let eps = f64::EPSILON;
        let transforms = [
            Transform::translate(1e9, 0.0),
            Transform::scale(1e-200, 1e-200),
            Transform::new(1.0 + eps, 1.0, 1.0, 1.0 - eps, 0.0, 0.0),
        ];
        assert_eq!(transforms[2].determinant(), 0.0); // Exact determinant is -eps^2.
        for transform in transforms {
            for rule in [FillRule::NonZero, FillRule::EvenOdd] {
                assert!(
                    outline(&path, transform, rule, &Limits::default())
                        .unwrap()
                        .is_none()
                );
            }
        }
    }

    #[test]
    fn certified_empty_fill_tiles_and_clip_match_full_coverage() {
        let mut path = PathBuilder::new();
        path.rect(3.0, 1.0, 0.0, 6.0);
        let mut path = path.finish();
        let paint = Paint::Solid(Rgba8::BLACK);
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            path.classify_fill(rule, &Limits::default(), &mut 0)
                .unwrap();
            let clip = Mask::from_path(8, 8, &path, Transform::IDENTITY, rule, 64).unwrap();
            let mut full = Pixmap::new(8, 8, Rgba8::TRANSPARENT, 64).unwrap();
            fill_path(&mut full, &path, Transform::IDENTITY, rule, &paint, None).unwrap();
            for y in 0..8 {
                for x in 0..8 {
                    let mut tile = Pixmap::new(1, 1, Rgba8::TRANSPARENT, 1).unwrap();
                    fill_path_at(
                        &mut tile,
                        &path,
                        Transform::IDENTITY,
                        rule,
                        &paint,
                        None,
                        (x, y),
                    )
                    .unwrap();
                    let offset = usize::try_from((y * 8 + x) * 4).unwrap();
                    assert_eq!(tile.data(), &full.data()[offset..offset + 4]);
                    let expected = if (2..4).contains(&x) && (1..7).contains(&y) {
                        128
                    } else {
                        0
                    };
                    assert_eq!(tile.data()[3], expected);
                    assert_eq!(clip.coverage()[(y * 8 + x) as usize], expected);
                }
            }
        }
    }

    #[test]
    fn point_parity_uses_cells_without_distinguishing_signed_zero() {
        let mut path = PathBuilder::new();
        path.rect(-3.5, -7.0, 0.0, 6.0);
        for (x, y) in [(0.0, -2.25), (0.0, -3.25), (-0.0, -2.25)] {
            path.move_to(Point::new(x, y)).close();
        }
        let transform = Transform::new(-1.0, -0.0, 0.0, -1.0, -0.0, 0.0);
        assert!(transform.apply(Point::new(0.0, -2.25)).x.is_sign_negative());
        assert!(
            transform
                .apply(Point::new(-0.0, -2.25))
                .x
                .is_sign_positive()
        );
        let mut target = Pixmap::new(8, 8, Rgba8::TRANSPARENT, 64).unwrap();
        let mut path = path.finish();
        path.classify_fill(FillRule::EvenOdd, &Limits::default(), &mut 0)
            .unwrap();
        fill_path(
            &mut target,
            &path,
            transform,
            FillRule::EvenOdd,
            &Paint::Solid(Rgba8::BLACK),
            None,
        )
        .unwrap();
        assert_eq!(target.data()[(2 * 8) * 4 + 3], 0);
        assert_eq!(target.data()[(3 * 8) * 4 + 3], 255);
    }

    #[test]
    fn ordinary_large_paths_only_charge_one_source_pass_and_cache() {
        let mut builder = PathBuilder::new();
        for i in 0..70_000 {
            let angle = f64::from(i) * std::f64::consts::TAU / 70_000.0;
            let point = Point::new(16.0 + 12.0 * angle.cos(), 16.0 + 12.0 * angle.sin());
            if i == 0 {
                builder.move_to(point);
            } else {
                builder.line_to(point).unwrap();
            }
        }
        let mut path = builder.finish();
        let mut work = 0;
        let limits = Limits {
            raster_work: 2_000_000,
            ..Limits::default()
        };
        path.classify_fill(FillRule::NonZero, &limits, &mut work)
            .unwrap();
        assert_eq!(
            path.fill_region(FillRule::NonZero),
            Some(FillRegion::Ordinary)
        );
        assert_eq!(work, 70_000);
        let previous = work;
        path.classify_fill(FillRule::NonZero, &limits, &mut work)
            .unwrap();
        assert_eq!(work, previous);
        assert!(
            outline(
                &path,
                Transform::IDENTITY,
                FillRule::NonZero,
                &Limits {
                    raster_work: 0,
                    ..limits
                }
            )
            .unwrap()
            .is_none()
        );
        let mut work = 0;
        path.classify_fill(FillRule::EvenOdd, &limits, &mut work)
            .unwrap();
        assert_eq!(work, 70_000);
        assert_eq!(
            path.fill_region(FillRule::EvenOdd),
            Some(FillRegion::Ordinary)
        );
    }

    #[test]
    fn ordinary_precheck_is_sufficient_for_both_rules() {
        let cases: &[&[(f64, f64)]] = &[
            &[(0.0, 0.0), (4.0, 4.0), (0.0, 4.0), (4.0, 0.0)], // bow tie
            &[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)],
            &[(0.0, 0.0), (4.0, 4.0), (2.0, 2.0)], // retracing
            &[
                (0.0, 0.0),
                (4.0, 0.0),
                (4.0, 4.0),
                (0.0, 4.0),
                (0.0, 0.0),
                (4.0, 0.0),
                (4.0, 4.0),
                (0.0, 4.0),
            ], // twice
        ];
        for (i, points) in cases.iter().enumerate() {
            let mut builder = PathBuilder::new();
            for (j, &(x, y)) in points.iter().enumerate() {
                if j == 0 {
                    builder.move_to(Point::new(x, y));
                } else {
                    builder.line_to(Point::new(x, y)).unwrap();
                }
            }
            builder.close();
            let path = builder.finish();
            assert_eq!(ordinary_polygon(&path).unwrap(), i < 2);
        }
        let mut builder = PathBuilder::new();
        builder.rect(0.0, 0.0, 4.0, 4.0).rect(0.0, 0.0, 4.0, 4.0);
        let mut path = builder.finish();
        assert!(!ordinary_polygon(&path).unwrap());
        path.classify_fill(FillRule::EvenOdd, &Limits::default(), &mut 0)
            .unwrap();
        assert_eq!(
            path.fill_region(FillRule::EvenOdd),
            Some(FillRegion::EmptyBoundary)
        );
    }

    #[test]
    fn random_oblique_retracing_has_direction_independent_keys_and_paints() {
        // Fixed seed, ten independent endpoint pairs; arbitrary binary
        // coordinates exercise rounded slopes and intercepts in both axes.
        let mut seed = 0x4c8b_3a29_720d_613f_u64;
        for _ in 0..10 {
            let mut coordinate = || {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                0.5 + (seed >> 11) as f64 * (6.0 / (1_u64 << 53) as f64)
            };
            let a = Point::new(coordinate(), coordinate());
            let b = Point::new(coordinate(), coordinate());
            assert_eq!(Edge::new(a, b).key, Edge::new(b, a).key);
            for (a, b) in [(a, b), (b, a)] {
                let mut builder = PathBuilder::new();
                builder.move_to(a).line_to(b).unwrap().close();
                let mut path = builder.finish();
                for rule in [FillRule::NonZero, FillRule::EvenOdd] {
                    path.classify_fill(rule, &Limits::default(), &mut 0)
                        .unwrap();
                    assert_eq!(path.fill_region(rule), Some(FillRegion::EmptyBoundary));
                    let mut target = Pixmap::new(8, 8, Rgba8::TRANSPARENT, 64).unwrap();
                    fill_path(
                        &mut target,
                        &path,
                        Transform::IDENTITY,
                        rule,
                        &Paint::Solid(Rgba8::BLACK),
                        None,
                    )
                    .unwrap();
                    assert!(target.data().as_chunks::<4>().0.iter().any(|p| p[3] != 0));
                }
            }
        }
    }

    fn polygons(contours: &[&[(f64, f64)]]) -> Path {
        let mut builder = PathBuilder::new();
        for contour in contours {
            builder.move_to(Point::new(contour[0].0, contour[0].1));
            for &(x, y) in &contour[1..] {
                builder.line_to(Point::new(x, y)).unwrap();
            }
            builder.close();
        }
        builder.finish()
    }

    /// Regions under both rules, and whether only the source pass was charged.
    fn classified(mut path: Path) -> [(FillRegion, bool); 2] {
        [FillRule::NonZero, FillRule::EvenOdd].map(|rule| {
            let mut work = 0;
            path.classify_fill(rule, &Limits::default(), &mut work)
                .unwrap();
            (
                path.fill_region(rule).unwrap(),
                work == path.segments().len(),
            )
        })
    }

    #[test]
    fn ordinary_precheck_settles_many_line_subpaths() {
        let mut rectangles = PathBuilder::new();
        let mut triangles = PathBuilder::new();
        for i in 0..1_000 {
            let (x, y) = (f64::from(i % 40) * 3.0, f64::from(i / 40) * 4.0);
            rectangles.rect(x, y, 2.0, 3.0);
            triangles
                .move_to(Point::new(x, y))
                .line_to(Point::new(x + 2.0, y + 0.5))
                .unwrap()
                .line_to(Point::new(x + 1.0, y + 3.0))
                .unwrap()
                .close();
        }
        // A repeated close adds only a zero-length edge.
        triangles.close();
        for path in [rectangles.finish(), triangles.finish()] {
            assert!(ordinary_polygon(&path).unwrap());
            assert_eq!(classified(path), [(FillRegion::Ordinary, true); 2]);
        }
    }

    #[test]
    fn shared_extreme_vertex_falls_through_to_full_classification() {
        let path = polygons(&[
            &[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0)],
            &[(4.0, 4.0), (0.0, 4.0), (2.0, 2.0)],
        ]);
        assert!(!ordinary_polygon(&path).unwrap());
        assert_eq!(classified(path), [(FillRegion::Ordinary, false); 2]);
    }

    #[test]
    fn empty_multiple_subpaths_stay_certified() {
        let triangle = [(0.0, 0.0), (4.0, 1.0), (1.0, 3.0)];
        let reversed = [(1.0, 3.0), (4.0, 1.0), (0.0, 0.0)];
        let width = [(1.0, 1.0), (1.0, 1.0), (1.0, 5.0), (1.0, 5.0)];
        let cases = [
            (polygons(&[&triangle, &reversed]), FillRegion::EmptyBoundary),
            (polygons(&[&triangle, &triangle]), FillRegion::Ordinary),
            (
                polygons(&[&width, &[(2.0, 0.0), (5.0, 3.0)]]),
                FillRegion::EmptyBoundary,
            ),
        ];
        for (path, nonzero) in cases {
            assert!(!ordinary_polygon(&path).unwrap());
            let [(a, _), (b, _)] = classified(path);
            // Twice in one direction has winding 2: nonzero fills, even-odd is empty.
            assert_eq!((a, b), (nonzero, FillRegion::EmptyBoundary));
        }
    }

    #[test]
    fn ordinary_precheck_never_contradicts_full_classification() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        // A small grid makes repeated vertices, collinear runs and retraces common.
        for _ in 0..20_000 {
            let mut builder = PathBuilder::new();
            for _ in 0..=next() % 3 {
                let mut points: Vec<_> = (0..2 + next() % 6)
                    .map(|_| Point::new((next() % 4) as f64, (next() % 4) as f64))
                    .collect();
                match next() % 3 {
                    0 => points.extend_from_within(..),
                    1 => points.extend(points.clone().into_iter().rev()),
                    _ => {}
                }
                builder.move_to(points[0]);
                for &point in &points[1..] {
                    builder.line_to(point).unwrap();
                }
                if next() % 2 == 0 {
                    builder.close();
                }
            }
            let path = builder.finish();
            if ordinary_polygon(&path).unwrap() {
                for rule in [FillRule::NonZero, FillRule::EvenOdd] {
                    let full = classify_uncertain(&path, rule, &Limits::default(), &mut 0);
                    assert_eq!(full.unwrap(), FillRegion::Ordinary, "{path:?}");
                }
            }
        }
    }

    #[test]
    fn ordinary_precheck_accepts_explicit_closure_without_hiding_retracing() {
        for twice in [false, true] {
            for close in [false, true] {
                let first = Point::new(4.0, 2.0);
                let mut builder = PathBuilder::new();
                builder.move_to(first);
                for _ in 0..if twice { 2 } else { 1 } {
                    builder.line_to(Point::new(0.0, 0.0)).unwrap();
                    builder.line_to(Point::new(0.0, 4.0)).unwrap();
                    builder.line_to(first).unwrap();
                }
                if close {
                    builder.close();
                }
                let mut path = builder.finish();
                assert_eq!(ordinary_polygon(&path).unwrap(), !twice);
                for rule in [FillRule::NonZero, FillRule::EvenOdd] {
                    let mut work = 0;
                    let limits = Limits {
                        raster_work: if twice {
                            usize::MAX
                        } else {
                            path.segments().len()
                        },
                        ..Limits::default()
                    };
                    path.classify_fill(rule, &limits, &mut work).unwrap();
                    assert_eq!(
                        path.fill_region(rule),
                        Some(if twice && rule == FillRule::EvenOdd {
                            FillRegion::EmptyBoundary
                        } else {
                            FillRegion::Ordinary
                        })
                    );
                    if !twice {
                        assert_eq!(work, path.segments().len());
                    }
                }
            }
        }
    }

    #[test]
    fn singular_stroke_outlines_keep_ordinary_scan_conversion() {
        let mut path = PathBuilder::new();
        path.rect(3.0, 1.0, 0.0, 6.0);
        let mut path = path.finish();
        path.classify_fill(FillRule::NonZero, &Limits::default(), &mut 0)
            .unwrap();
        assert_eq!(
            path.fill_region(FillRule::NonZero),
            Some(FillRegion::EmptyBoundary)
        );
        let transform = Transform::new(1.0, 0.0, 0.0, 0.0, 0.0, 3.0);
        let mut target = Pixmap::new(8, 8, Rgba8::TRANSPARENT, 64).unwrap();
        crate::stroke_path(
            &mut target,
            &path,
            transform,
            &Stroke {
                width: 2.0,
                ..Stroke::default()
            },
            &Paint::Solid(Rgba8::BLACK),
            None,
        )
        .unwrap();
        assert!(target.data().iter().all(|&v| v == 0));
    }

    #[test]
    fn prepared_points_keep_floor_cells_and_original_segment_limits() {
        let limits = Limits {
            path_segments: 2,
            ..Limits::default()
        };
        let mut builder = PathBuilder::new();
        builder.move_to(Point::new(3.25, 2.25)).close();
        let mut path = builder.finish();
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            path.classify_fill(rule, &limits, &mut 0).unwrap();
            assert_eq!(path.fill_region(rule), Some(FillRegion::Points));
            assert!(
                outline(&path, Transform::IDENTITY, rule, &limits)
                    .unwrap()
                    .is_none()
            );
            let mut target = Pixmap::new_with_limits(8, 8, Rgba8::TRANSPARENT, limits).unwrap();
            fill_path(
                &mut target,
                &path,
                Transform::IDENTITY,
                rule,
                &Paint::Solid(Rgba8::BLACK),
                None,
            )
            .unwrap();
            assert_eq!(
                target
                    .data()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|p| p[3] != 0)
                    .count(),
                1
            );
            assert_eq!(target.data()[(2 * 8 + 3) * 4 + 3], 255);
        }
    }

    #[test]
    fn boundary_classification_has_a_work_budget() {
        let mut path = PathBuilder::new();
        path.rect(3.5, 1.0, 0.0, 6.0);
        let limits = Limits {
            raster_work: 0,
            ..Limits::default()
        };
        assert!(
            path.finish()
                .classify_fill(FillRule::NonZero, &limits, &mut 0)
                .is_err()
        );
        let mut builder = PathBuilder::new();
        builder.rect(0.0, 0.0, 4.0, 4.0);
        let mut path = builder.finish();
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            let limits = Limits {
                raster_work: 5,
                ..Limits::default()
            };
            let mut work = 1;
            assert!(path.classify_fill(rule, &limits, &mut work).is_err());
            let mut work = 0;
            path.classify_fill(rule, &limits, &mut work).unwrap();
            assert_eq!(work, 5);
        }
        // A twice-traced rectangle cannot pass the even-odd precheck. Its
        // second source pass must also fit the shared page work budget.
        let mut builder = PathBuilder::new();
        builder.rect(0.0, 0.0, 4.0, 4.0).rect(0.0, 0.0, 4.0, 4.0);
        let mut path = builder.finish();
        let limits = Limits {
            raster_work: path.segments().len(),
            ..Limits::default()
        };
        let mut work = 0;
        assert!(
            path.classify_fill(FillRule::EvenOdd, &limits, &mut work)
                .is_err()
        );
        assert_eq!(work, limits.raster_work);
    }
}
