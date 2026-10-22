//! Closed-form coverage of axis-aligned rectangles (8.5.3–8.5.4): a row's
//! coverage is its covered height times each column's covered width.

use super::edges::{Bounds, Polygon};
use super::row::Sink;
use crate::quantize_unit;

/// The rectangle a polygon of one axis-aligned four-sided contour encloses.
pub(crate) fn rectangle(polygon: &Polygon) -> Option<Bounds> {
    let mut contours = polygon.contours();
    let ([a, b, c, d], None) = (contours.next()?, contours.next()) else {
        return None;
    };
    let (a, b, c, d) = (*a, *b, *c, *d);
    let axis = (a.x == b.x && b.y == c.y && c.x == d.x && d.y == a.y)
        || (a.y == b.y && b.x == c.x && c.y == d.y && d.x == a.x);
    (axis && a != b && b != c && c != d && d != a).then_some(Bounds {
        left: a.x.min(c.x),
        top: a.y.min(c.y),
        right: a.x.max(c.x),
        bottom: a.y.max(c.y),
    })
}

/// Emits row `y` of `rect`, which lies within the region, to `sink`.
/// Returns the runs emitted.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the rectangle lies within the u32 region"
)]
pub(crate) fn emit_row(rect: Bounds, y: u32, sink: &mut impl Sink) -> usize {
    let top = f64::from(y);
    let height = (rect.bottom.min(top + 1.0) - rect.top.max(top)).clamp(0.0, 1.0);
    if height <= 0.0 || rect.left >= rect.right {
        return 0;
    }
    let start = rect.left.floor();
    let end = rect.right.ceil();
    let first = (rect.right.min(start + 1.0) - rect.left).min(1.0);
    let last = rect.right - (end - 1.0).max(rect.left);
    let (start, end) = (start as usize, end as usize);
    if end <= start {
        return 0;
    }
    if end == start + 1 {
        sink.run(start, end, quantize_unit(last * height));
        return 1;
    }
    sink.run(start, start + 1, quantize_unit(first * height));
    sink.run(start + 1, end - 1, quantize_unit(height));
    sink.run(end - 1, end, quantize_unit(last * height));
    3
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Point;
    use pagura_core::Limits;

    struct Collect(Vec<(usize, usize, u8)>);
    impl Sink for Collect {
        fn run(&mut self, x0: usize, x1: usize, value: u8) {
            if x0 < x1 {
                self.0.push((x0, x1, value));
            }
        }
    }

    #[test]
    fn rows_take_height_times_width() {
        let rect = Bounds {
            left: 1.25,
            top: 0.5,
            right: 4.5,
            bottom: 3.0,
        };
        let mut out = Collect(Vec::new());
        emit_row(rect, 0, &mut out);
        assert_eq!(out.0, [(1, 2, 96), (2, 4, 128), (4, 5, 64)]);
        out.0.clear();
        emit_row(rect, 1, &mut out);
        assert_eq!(out.0, [(1, 2, 191), (2, 4, 255), (4, 5, 128)]);
        out.0.clear();
        emit_row(rect, 3, &mut out);
        assert_eq!(out.0, [] as [(usize, usize, u8); 0]);
    }

    #[test]
    fn only_single_axis_aligned_quadrilaterals_are_rectangles() {
        let mut polygon = Polygon::default();
        polygon.begin(&Limits::default(), 4).unwrap();
        polygon.points.extend([
            Point::new(0.0, 0.0),
            Point::new(2.0, 0.0),
            Point::new(2.0, 1.0),
            Point::new(0.0, 1.0),
        ]);
        polygon.end();
        assert!(rectangle(&polygon).is_some());
        polygon.points[2].x = 2.5;
        assert!(rectangle(&polygon).is_none());
    }
}
