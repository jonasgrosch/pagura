//! Points, vectors and affine transforms.

use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// A point, or the vector from the origin to it, in a two-dimensional
/// coordinate space.
///
/// One type serves both roles, as in PDF's own notation: `b - a` is the
/// vector from `a` to `b`, and `a + v` moves `a` by `v`.
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

    /// The dot product of two vectors.
    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y
    }

    /// The z component of the cross product of two vectors: positive when
    /// `other` turns counterclockwise from `self` in a y-up space.
    pub fn cross(self, other: Self) -> f64 {
        self.x * other.y - self.y * other.x
    }

    /// The Euclidean length of a vector, without intermediate overflow.
    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }

    /// The vector rotated a quarter turn counterclockwise (y-up).
    #[must_use]
    pub fn perp(self) -> Self {
        Self::new(-self.y, self.x)
    }

    /// The point at parameter `t` on the line from `self` (`t = 0`) to
    /// `other` (`t = 1`).
    #[must_use]
    pub fn lerp(self, other: Self, t: f64) -> Self {
        self + (other - self) * t
    }

    /// The point halfway between `self` and `other`, without intermediate
    /// overflow.
    #[must_use]
    pub fn midpoint(self, other: Self) -> Self {
        Self::new(self.x.midpoint(other.x), self.y.midpoint(other.y))
    }
}

impl Add for Point {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y)
    }
}

impl AddAssign for Point {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Point {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y)
    }
}

impl SubAssign for Point {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}

impl Neg for Point {
    type Output = Self;

    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

/// Scales a vector.
impl Mul<f64> for Point {
    type Output = Self;

    fn mul(self, factor: f64) -> Self {
        Self::new(self.x * factor, self.y * factor)
    }
}

/// Divides a vector by a scalar.
impl Div<f64> for Point {
    type Output = Self;

    fn div(self, divisor: f64) -> Self {
        Self::new(self.x / divisor, self.y / divisor)
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

/// The sign of every turn of the closed polygon `points` when they certify
/// a simple convex polygon: strict turns of one sign and exactly one
/// revolution. The error guard excludes uncertain determinants, including
/// underflowing ones, and collinear subdivision.
pub(crate) fn convex_orientation(points: &[Point]) -> Option<f64> {
    let mut sign = 0.0_f64;
    let mut turns = 0;
    let half = |p: Point| p.y > 0.0 || (p.y == 0.0 && p.x >= 0.0);
    for i in 0..points.len() {
        let p = points[if i == 0 { points.len() - 1 } else { i - 1 }];
        let q = points[i];
        let r = points[(i + 1) % points.len()];
        let a = q - p;
        let b = r - q;
        turns += usize::from(half(a) != half(b));
        let (left, right) = (a.x * b.y, a.y * b.x);
        let cross = left - right;
        if cross.abs() <= 8.0 * f64::EPSILON * (left.abs() + right.abs()) + f64::MIN_POSITIVE
            || (sign != 0.0 && cross.signum() != sign)
        {
            return None;
        }
        sign = cross.signum();
    }
    (turns == 2).then_some(sign)
}

#[cfg(test)]
mod tests {
    use super::{Point, Transform, convex_orientation};

    #[test]
    fn vector_operators_match_componentwise_arithmetic() {
        let a = Point::new(3.0, -4.0);
        let b = Point::new(-1.5, 2.0);
        assert_eq!(a + b, Point::new(1.5, -2.0));
        assert_eq!(a - b, Point::new(4.5, -6.0));
        assert_eq!(-a, Point::new(-3.0, 4.0));
        assert_eq!(a * 2.0, Point::new(6.0, -8.0));
        assert_eq!(a / 2.0, Point::new(1.5, -2.0));
        let mut c = a;
        c += b;
        c -= a;
        assert_eq!(c, b);
        assert_eq!(a.dot(b), -12.5);
        assert_eq!(a.cross(b), 0.0);
        assert_eq!(Point::new(1.0, 0.0).cross(Point::new(0.0, 1.0)), 1.0);
        assert_eq!(a.length(), 5.0);
        assert_eq!(Point::new(1.0, 2.0).perp(), Point::new(-2.0, 1.0));
        assert_eq!(a.lerp(b, 0.0), a);
        assert_eq!(a.lerp(b, 1.0), b);
        assert_eq!(a.lerp(b, 0.25), Point::new(1.875, -2.5));
        assert_eq!(a.midpoint(b), Point::new(0.75, -1.0));
        // Squares and sums of these coordinates overflow f64; the results do not.
        let length = Point::new(3e200, 4e200).length();
        assert!((length / 5e200 - 1.0).abs() < 1e-15);
        let huge = Point::new(f64::MAX, -f64::MAX);
        assert_eq!(huge.midpoint(huge), huge);
    }

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

    #[test]
    fn rejects_uncertain_concave_and_repeated_turns() {
        let rectangle = [
            Point::new(1.0, 1.0),
            Point::new(7.0, 1.0),
            Point::new(7.0, 7.0),
            Point::new(1.0, 7.0),
        ];
        assert_eq!(convex_orientation(&rectangle), Some(1.0));
        assert_eq!(
            convex_orientation(&rectangle.into_iter().rev().collect::<Vec<_>>()),
            Some(-1.0)
        );
        assert!(convex_orientation(&rectangle.repeat(2)).is_none());
        let mut concave = rectangle.to_vec();
        concave.insert(2, Point::new(2.0, 2.0));
        assert!(convex_orientation(&concave).is_none());
        let mut subdivided = rectangle.to_vec();
        subdivided.insert(1, Point::new(4.0, 1.0));
        assert!(convex_orientation(&subdivided).is_none());
        let star: Vec<_> = (0..5)
            .map(|i| {
                let a = f64::from((i * 2) % 5) * std::f64::consts::TAU / 5.0;
                Point::new(a.cos(), a.sin())
            })
            .collect();
        assert!(convex_orientation(&star).is_none());
    }
}
