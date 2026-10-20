//! Source footprints of device pixels on the original sample grid.
use super::index;

/// The source cells under one device pixel along one axis: cells
/// `first..end`, weighted `head`, `middle` (each interior cell) and `tail`,
/// normalized to sum to one. Bilinear footprints have two cells.
#[derive(Clone, Copy, Debug)]
pub(super) struct Footprint {
    pub(super) first: usize,
    pub(super) end: usize,
    pub(super) head: f64,
    pub(super) tail: f64,
    pub(super) middle: f64,
}
impl Footprint {
    pub(super) fn new(pixel: usize, scale: f64, offset: f64, maximum: usize, linear: bool) -> Self {
        let a = pixel as f64 * scale + offset;
        let b = (pixel as f64 + 1.0) * scale + offset;
        if linear {
            let centre = a.midpoint(b) - 0.5;
            let first = index(centre.floor()).min(maximum - 1);
            let last = index(centre.floor() + 1.0).min(maximum - 1);
            if first == last {
                return Self {
                    first,
                    end: first + 1,
                    head: 1.0,
                    tail: 1.0,
                    middle: 0.0,
                };
            }
            let fraction = centre - centre.floor();
            return Self {
                first,
                end: last + 1,
                head: 1.0 - fraction,
                tail: fraction,
                middle: 0.0,
            };
        }
        let low = a.min(b).clamp(0.0, maximum as f64);
        let high = a.max(b).clamp(0.0, maximum as f64);
        if high <= low {
            let first = index(a.midpoint(b)).min(maximum - 1);
            return Self {
                first,
                end: first + 1,
                head: 1.0,
                tail: 1.0,
                middle: 0.0,
            };
        }
        let first = index(low.floor()).min(maximum - 1);
        let end = index(high.ceil()).min(maximum);
        let denominator = high - low;
        Self {
            first,
            end,
            head: ((first + 1) as f64).min(high) - low,
            tail: high - ((end - 1) as f64).max(low),
            middle: 1.0,
        }
        .normalized(denominator)
    }
    fn normalized(mut self, denominator: f64) -> Self {
        self.head /= denominator;
        self.tail /= denominator;
        self.middle /= denominator;
        self
    }
    pub(super) fn weight(self, index: usize) -> f64 {
        if index == self.first {
            self.head
        } else if index == self.end - 1 {
            self.tail
        } else {
            self.middle
        }
    }
}

/// A source sample and its normalized footprint weight.
#[derive(Debug, Clone, Copy)]
pub(super) struct Tap {
    /// Index in a source row.
    pub(super) sample: usize,
    /// Weight of the source cell in the destination footprint.
    pub(super) weight: f32,
}
/// The tap interval of one destination pixel.
#[derive(Debug, Clone, Copy)]
pub(super) struct Band {
    /// First tap, inclusive.
    pub(super) start: usize,
    /// Last tap, exclusive.
    pub(super) end: usize,
}
