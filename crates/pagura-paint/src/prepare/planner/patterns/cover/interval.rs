//! Finite outward binary-f64 intervals for the constant-cell witness proof.

#[derive(Debug, Clone, Copy)]
pub(super) struct Interval {
    lo: f64,
    hi: f64,
}

impl Interval {
    pub(super) fn new(lo: f64, hi: f64) -> Option<Self> {
        (lo.is_finite() && hi.is_finite() && lo <= hi).then_some(Self { lo, hi })
    }

    pub(super) fn point(value: f64) -> Option<Self> {
        Self::new(value, value)
    }
    pub(super) fn lower(self) -> f64 {
        self.lo
    }
    pub(super) fn upper(self) -> f64 {
        self.hi
    }

    pub(super) fn add(self, rhs: Self) -> Option<Self> {
        Self::new((self.lo + rhs.lo).next_down(), (self.hi + rhs.hi).next_up())
    }

    pub(super) fn sub(self, rhs: Self) -> Option<Self> {
        Self::new((self.lo - rhs.hi).next_down(), (self.hi - rhs.lo).next_up())
    }

    pub(super) fn mul(self, rhs: Self) -> Option<Self> {
        Self::operations([
            self.lo * rhs.lo,
            self.lo * rhs.hi,
            self.hi * rhs.lo,
            self.hi * rhs.hi,
        ])
    }

    pub(super) fn div(self, rhs: Self) -> Option<Self> {
        if rhs.lo <= 0.0 && rhs.hi >= 0.0 {
            return None;
        }
        Self::operations([
            self.lo / rhs.lo,
            self.lo / rhs.hi,
            self.hi / rhs.lo,
            self.hi / rhs.hi,
        ])
    }

    fn operations(values: [f64; 4]) -> Option<Self> {
        if !values.iter().all(|v| v.is_finite()) {
            return None;
        }
        let lo = values.into_iter().fold(f64::INFINITY, f64::min).next_down();
        let hi = values
            .into_iter()
            .fold(f64::NEG_INFINITY, f64::max)
            .next_up();
        Self::new(lo, hi)
    }
}
