//! Points and affine transforms.

use std::ops::Mul;

/// A point in a two-dimensional coordinate space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    /// Horizontal coordinate.
    pub x: f64,
    /// Vertical coordinate.
    pub y: f64,
}

impl Point {
    /// Creates a point.
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Whether both coordinates are finite.
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// An affine transform `[a b c d e f]` in PDF's row-vector convention
/// (ISO 32000-2 clause 8.3.4): `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Horizontal scaling and rotation component.
    pub a: f64,
    /// Rotation and skew component.
    pub b: f64,
    /// Rotation and skew component.
    pub c: f64,
    /// Vertical scaling and rotation component.
    pub d: f64,
    /// Horizontal translation.
    pub e: f64,
    /// Vertical translation.
    pub f: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    /// The identity transform.
    pub const IDENTITY: Self = Self::new(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);

    /// Creates a transform from its six components.
    pub const fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Self {
        Self { a, b, c, d, e, f }
    }

    /// A translation.
    pub const fn translate(x: f64, y: f64) -> Self {
        Self::new(1.0, 0.0, 0.0, 1.0, x, y)
    }

    /// An axis-aligned scaling.
    pub const fn scale(x: f64, y: f64) -> Self {
        Self::new(x, 0.0, 0.0, y, 0.0, 0.0)
    }

    /// The transform that applies `self` first and then `next`.
    ///
    /// PDF's `cm` operator computes `CTM' = M × CTM`, which is
    /// `m.then(ctm)`.
    #[must_use]
    pub fn then(self, next: Self) -> Self {
        Self {
            a: self.a * next.a + self.b * next.c,
            b: self.a * next.b + self.b * next.d,
            c: self.c * next.a + self.d * next.c,
            d: self.c * next.b + self.d * next.d,
            e: self.e * next.a + self.f * next.c + next.e,
            f: self.e * next.b + self.f * next.d + next.f,
        }
    }

    /// Maps a point.
    pub fn apply(self, point: Point) -> Point {
        Point {
            x: self.a * point.x + self.c * point.y + self.e,
            y: self.b * point.x + self.d * point.y + self.f,
        }
    }

    /// The determinant of the linear part.
    pub fn determinant(self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    /// The inverse transform, or `None` if this transform is singular or
    /// not finite.
    pub fn inverse(self) -> Option<Self> {
        let det = self.determinant();
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let a = self.d / det;
        let b = -self.b / det;
        let c = -self.c / det;
        let d = self.a / det;
        let inverse = Self {
            a,
            b,
            c,
            d,
            e: -(self.e * a + self.f * c),
            f: -(self.e * b + self.f * d),
        };
        inverse.is_finite().then_some(inverse)
    }

    /// Whether every component is finite.
    pub fn is_finite(self) -> bool {
        [self.a, self.b, self.c, self.d, self.e, self.f]
            .iter()
            .all(|v| v.is_finite())
    }
}

/// `p * t` maps `p` by `t`, matching PDF's row-vector notation.
impl Mul<Transform> for Point {
    type Output = Point;

    fn mul(self, transform: Transform) -> Point {
        transform.apply(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{Point, Transform};

    #[test]
    fn then_applies_left_operand_first() {
        let scale = Transform::scale(2.0, 3.0);
        let shift = Transform::translate(10.0, 20.0);
        assert_eq!(
            Point::new(1.0, 1.0) * scale.then(shift),
            Point::new(12.0, 23.0)
        );
        assert_eq!(
            Point::new(1.0, 1.0) * shift.then(scale),
            Point::new(22.0, 63.0)
        );
    }

    #[test]
    fn inverse_round_trips_and_rejects_singular_transforms() {
        let transform = Transform::new(0.0, 2.0, -3.0, 0.0, 5.0, 7.0);
        let inverse = transform.inverse().unwrap();
        let point = Point::new(4.0, -1.5);
        let back = point * transform * inverse;
        assert!((back.x - point.x).abs() < 1e-12 && (back.y - point.y).abs() < 1e-12);
        assert!(Transform::scale(0.0, 1.0).inverse().is_none());
    }
}
