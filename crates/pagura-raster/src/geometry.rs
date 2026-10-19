//! Points, vectors, affine transforms and device pixel rectangles.

use pagura_core::{Error, Result};
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
        self.x += other.x;
        self.y += other.y;
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
        self.x -= other.x;
        self.y -= other.y;
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

/// A non-empty rectangle of whole device pixels: columns `x..x + width`
/// and rows `y..y + height`, with both far edges within `u32`.
///
/// It places a mask, tile or clip in global device space, so that tiles of
/// one page integrate identical geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IntRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl IntRect {
    /// Creates a rectangle from its top-left pixel and its size.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` for a zero width or height and `Limit` if
    /// the right or bottom edge exceeds `u32`.
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(Error::invalid_argument(
                "pixel rectangle dimensions must be positive",
            ));
        }
        if x.checked_add(width).is_none() || y.checked_add(height).is_none() {
            return Err(Error::limit("pixel rectangle exceeds u32 coordinates"));
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    /// The rectangle `x0..x1` × `y0..y1`, or `None` if it is empty.
    pub(crate) fn from_edges(x0: u32, y0: u32, x1: u32, y1: u32) -> Option<Self> {
        (x0 < x1 && y0 < y1).then(|| Self {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        })
    }

    /// The leftmost column.
    pub const fn x(self) -> u32 {
        self.x
    }

    /// The top row.
    pub const fn y(self) -> u32 {
        self.y
    }

    /// The number of columns, at least 1.
    pub const fn width(self) -> u32 {
        self.width
    }

    /// The number of rows, at least 1.
    pub const fn height(self) -> u32 {
        self.height
    }

    /// The column just right of the rectangle.
    pub const fn right(self) -> u32 {
        // Cannot overflow: `new` and `from_edges` keep the edge within u32.
        self.x + self.width
    }

    /// The row just below the rectangle.
    pub const fn bottom(self) -> u32 {
        self.y + self.height
    }

    /// The number of pixels, which always fits `u64`.
    pub(crate) fn pixels(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
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
    use super::{IntRect, Point, Transform};
    use pagura_core::ErrorKind;

    #[test]
    fn pixel_rectangles_are_non_empty_and_within_u32() {
        let area = IntRect::new(10, 20, 3, 4).unwrap();
        assert_eq!(
            (
                area.x(),
                area.y(),
                area.right(),
                area.bottom(),
                area.pixels()
            ),
            (10, 20, 13, 24, 12)
        );
        let edge = IntRect::new(u32::MAX - 1, 0, 1, u32::MAX).unwrap();
        assert_eq!((edge.right(), edge.bottom()), (u32::MAX, u32::MAX));
        assert_eq!(
            IntRect::new(0, 0, 0, 1).unwrap_err().kind,
            ErrorKind::InvalidArgument
        );
        assert_eq!(
            IntRect::new(0, 0, 1, 0).unwrap_err().kind,
            ErrorKind::InvalidArgument
        );
        assert_eq!(
            IntRect::new(u32::MAX, 0, 1, 1).unwrap_err().kind,
            ErrorKind::Limit
        );
        assert_eq!(
            IntRect::new(0, 1, 1, u32::MAX).unwrap_err().kind,
            ErrorKind::Limit
        );
        assert_eq!(IntRect::from_edges(1, 1, 1, 2), None);
        assert_eq!(
            IntRect::from_edges(1, 1, 2, 2),
            IntRect::new(1, 1, 1, 1).ok()
        );
    }

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
}
