//! Axis-aligned rectangles among clipping paths (8.5.4): they clip a mark's
//! edges instead of being classified row by row.

use super::edges::{Bounds, Polygon};
use crate::Point;

/// The rectangle a polygon of one contour encloses when the contour is an
/// axis-aligned rectangle once the vertices that lie on a straight
/// axis-aligned run between their neighbours are dropped.
pub(crate) fn rectangle(polygon: &Polygon) -> Option<Bounds> {
    let mut contours = polygon.contours();
    let (Some(contour), None) = (contours.next(), contours.next()) else {
        return None;
    };
    let n = contour.len();
    let mut corners = [Point::default(); 4];
    let mut count = 0;
    for (i, &p) in contour.iter().enumerate() {
        let a = contour[(i + n - 1) % n];
        let b = contour[(i + 1) % n];
        if on_run(a, p, b) {
            continue;
        }
        *corners.get_mut(count)? = p;
        count += 1;
    }
    let [a, b, c, d] = corners;
    let axis = (a.x == b.x && b.y == c.y && c.x == d.x && d.y == a.y)
        || (a.y == b.y && b.x == c.x && c.y == d.y && d.x == a.x);
    (count == 4 && axis && a != b && b != c && c != d && d != a).then_some(Bounds {
        left: a.x.min(c.x),
        top: a.y.min(c.y),
        right: a.x.max(c.x),
        bottom: a.y.max(c.y),
    })
}

/// Whether `p` lies on the axis-aligned segment from `a` to `b`, so that
/// dropping it leaves the contour's region as it is.
fn on_run(a: Point, p: Point, b: Point) -> bool {
    let between = |u: f64, v: f64, w: f64| (u <= v && v <= w) || (w <= v && v <= u);
    (a.x == p.x && p.x == b.x && between(a.y, p.y, b.y))
        || (a.y == p.y && p.y == b.y && between(a.x, p.x, b.x))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::Limits;

    fn polygon(points: &[(f64, f64)]) -> Polygon {
        let mut polygon = Polygon::default();
        polygon.begin(&Limits::default(), points.len()).unwrap();
        polygon
            .points
            .extend(points.iter().map(|&(x, y)| Point::new(x, y)));
        polygon.end();
        polygon
    }

    #[test]
    fn only_single_axis_aligned_quadrilaterals_are_rectangles() {
        let mut square = polygon(&[(0.0, 0.0), (2.0, 0.0), (2.0, 1.0), (0.0, 1.0)]);
        assert!(rectangle(&square).is_some());
        square.points[2].x = 2.5;
        assert!(rectangle(&square).is_none());
    }

    #[test]
    fn vertices_on_the_sides_leave_a_rectangle() {
        // 0000266p22 writes its chart frame with two extra side points.
        let framed = polygon(&[
            (770.64, 445.92),
            (770.64, 123.48),
            (70.8, 123.48),
            (70.8, 284.7),
            (70.8, 445.92),
            (124.63, 445.92),
        ]);
        let r = rectangle(&framed).unwrap();
        assert_eq!(
            (r.left, r.top, r.right, r.bottom),
            (70.8, 123.48, 770.64, 445.92)
        );
        // A notch is not on a run.
        let notched = polygon(&[
            (0.0, 0.0),
            (4.0, 0.0),
            (4.0, 4.0),
            (2.0, 4.0),
            (2.0, 3.0),
            (0.0, 3.0),
        ]);
        assert!(rectangle(&notched).is_none());
    }
}
