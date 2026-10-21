//! PDF rectangles (ISO 32000-2 clause 7.9.5).

use crate::Value;

/// A normalized rectangle: `x0 <= x1` and `y0 <= y1`, all finite.
///
/// The fields are private so the invariant holds for every value:
/// [`Rect::from_corners`] and [`Rect::from_array`] are the only constructors,
/// and every operation preserves it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Rect {
    /// Creates a rectangle from two opposite corners, normalizing their
    /// order. Returns `None` if any coordinate is not finite.
    pub const fn from_corners(x0: f64, y0: f64, x1: f64, y1: f64) -> Option<Self> {
        if x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite() {
            Some(Self {
                x0: x0.min(x1),
                y0: y0.min(y1),
                x1: x0.max(x1),
                y1: y0.max(y1),
            })
        } else {
            None
        }
    }

    /// Reads a PDF rectangle array `[llx lly urx ury]`. Clause 7.9.5 lets
    /// writers give any two opposite corners, so the result is normalized.
    pub fn from_array(values: &[Value]) -> Option<Self> {
        match values {
            [x0, y0, x1, y1] => Self::from_corners(
                x0.as_number()?,
                y0.as_number()?,
                x1.as_number()?,
                y1.as_number()?,
            ),
            _ => None,
        }
    }

    /// Left edge, the smaller x coordinate.
    pub const fn x0(&self) -> f64 {
        self.x0
    }

    /// Bottom edge, the smaller y coordinate.
    pub const fn y0(&self) -> f64 {
        self.y0
    }

    /// Right edge, the larger x coordinate.
    pub const fn x1(&self) -> f64 {
        self.x1
    }

    /// Top edge, the larger y coordinate.
    pub const fn y1(&self) -> f64 {
        self.y1
    }

    /// Horizontal extent. Infinite if the edges are too far apart for `f64`.
    pub const fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    /// Vertical extent. Infinite if the edges are too far apart for `f64`.
    pub const fn height(&self) -> f64 {
        self.y1 - self.y0
    }

    /// Whether the rectangle encloses no area.
    pub const fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    /// The overlap of two rectangles, or `None` if they do not overlap.
    pub const fn intersection(&self, other: &Self) -> Option<Self> {
        let rect = Self {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        };
        if rect.is_empty() { None } else { Some(rect) }
    }

    /// The smallest rectangle that contains both.
    #[must_use]
    pub const fn union(&self, other: &Self) -> Self {
        Self {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Rect;
    use crate::Value;

    fn corners(rect: Rect) -> [f64; 4] {
        [rect.x0(), rect.y0(), rect.x1(), rect.y1()]
    }

    #[test]
    fn arrays_are_normalized_and_validated() {
        let values = [48, 48, 16, 32].map(Value::Integer);
        let rect = Rect::from_array(&values).unwrap();
        assert_eq!(corners(rect), [16.0, 32.0, 48.0, 48.0]);
        assert_eq!((rect.width(), rect.height()), (32.0, 16.0));
        assert!(Rect::from_array(&values[..3]).is_none());
        assert!(
            Rect::from_array(&[Value::Real(f64::NAN), Value::Null, Value::Null, Value::Null])
                .is_none()
        );
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(Rect::from_corners(0.0, 0.0, bad, 1.0).is_none());
        }
    }

    #[test]
    fn intersection_of_disjoint_rectangles_is_none() {
        let a = Rect::from_corners(0.0, 0.0, 10.0, 10.0).unwrap();
        let b = Rect::from_corners(10.0, 0.0, 20.0, 10.0).unwrap();
        assert!(a.intersection(&b).is_none());
        assert_eq!(a.intersection(&a), Some(a));
    }

    #[test]
    fn union_contains_both_rectangles() {
        let a = Rect::from_corners(0.0, 5.0, 10.0, 10.0).unwrap();
        let b = Rect::from_corners(-4.0, 0.0, 2.0, 7.0).unwrap();
        assert_eq!(corners(a.union(&b)), [-4.0, 0.0, 10.0, 10.0]);
        assert_eq!(a.union(&b), b.union(&a));
        let empty = Rect::from_corners(1.0, 1.0, 1.0, 3.0).unwrap();
        assert!(empty.is_empty());
        assert_eq!(a.union(&empty), a.union(&empty).union(&a));
    }
}
