//! PDF rectangles (ISO 32000-2 clause 7.9.5).

use crate::Value;

/// A normalized rectangle: `x0 <= x1` and `y0 <= y1`, all finite.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x0: f64,
    /// Bottom edge.
    pub y0: f64,
    /// Right edge.
    pub x1: f64,
    /// Top edge.
    pub y1: f64,
}

impl Rect {
    /// Creates a rectangle from two opposite corners, normalizing their
    /// order. Returns `None` if any coordinate is not finite.
    pub fn from_corners(x0: f64, y0: f64, x1: f64, y1: f64) -> Option<Self> {
        [x0, y0, x1, y1]
            .iter()
            .all(|v| v.is_finite())
            .then(|| Self {
                x0: x0.min(x1),
                y0: y0.min(y1),
                x1: x0.max(x1),
                y1: y0.max(y1),
            })
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

    /// Horizontal extent.
    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    /// Vertical extent.
    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }

    /// Whether the rectangle encloses no area.
    pub fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    /// The overlap of two rectangles, or `None` if they do not overlap.
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let rect = Self {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        };
        (!rect.is_empty()).then_some(rect)
    }
}

#[cfg(test)]
mod tests {
    use super::Rect;
    use crate::Value;

    #[test]
    fn arrays_are_normalized_and_validated() {
        let values = [48, 48, 16, 32].map(Value::Integer);
        let rect = Rect::from_array(&values).unwrap();
        assert_eq!(
            (rect.x0, rect.y0, rect.x1, rect.y1),
            (16.0, 32.0, 48.0, 48.0)
        );
        assert!(Rect::from_array(&values[..3]).is_none());
        assert!(
            Rect::from_array(&[Value::Real(f64::NAN), Value::Null, Value::Null, Value::Null])
                .is_none()
        );
    }

    #[test]
    fn intersection_of_disjoint_rectangles_is_none() {
        let a = Rect::from_corners(0.0, 0.0, 10.0, 10.0).unwrap();
        let b = Rect::from_corners(10.0, 0.0, 20.0, 10.0).unwrap();
        assert!(a.intersection(&b).is_none());
        assert_eq!(a.intersection(&a), Some(a));
    }
}
