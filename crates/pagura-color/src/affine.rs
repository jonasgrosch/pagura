//! Affine enclosures of the values an exact evaluation computes, for every
//! input of an interval at once (affine arithmetic, after Comba and Stolfi).
//!
//! A form `c + s·ξ ± r` stands for one value of the evaluation: each input
//! of the interval is `c₀ + s₀·ξ` for some `ξ` in `[−1, 1]` (the same `ξ` for
//! every form of that evaluation), and the value the evaluation computes for
//! that input, rounding as it rounds, lies within `r` of `c + s·ξ`. Each
//! operation below follows one rounded operation of the evaluation. The
//! value of `x op y` is within `u·|x op y|` of the exact result (`u = 2⁻⁵³`,
//! round to nearest), and the form's own `c` and `s` are rounded once each,
//! so a result's radius grows by at most `2u` times its magnitude
//! `|c| + |s| + r`; [`grow`] covers the rounding of the radius itself and
//! subnormal results.
//!
//! A form also holds bounds `[lo, hi]` of its values in IEEE total order
//! (`−0` below `+0`), found by the same rounded operation on the operands'
//! bounds: sums, differences and products by a constant are monotone in
//! that order, so the evaluation's value lies between them. They decide
//! what the line cannot near its ends, and bounds that meet give the value
//! bit for bit, the sign of a zero included.
//!
//! Discrete choices (a grid cell, a subdomain, a branch) must be the same
//! for every input; where they are not, the evaluation stops with the form
//! and threshold that decide them, and its caller splits the interval
//! there.

/// Twice the unit roundoff: the relative error of one rounded operation in
/// a value and in the form's coefficients together.
const EPS: f64 = f64::EPSILON;

/// An affine enclosure `c + s·ξ ± r` with bounds `[lo, hi]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Affine {
    pub(crate) c: f64,
    pub(crate) s: f64,
    pub(crate) r: f64,
    lo: f64,
    hi: f64,
}

/// Why an evaluation could not enclose an interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Undecided {
    /// A choice changes inside the interval, where `form` reaches `at`, and
    /// for grid cells at each of the `count − 1` boundaries after it, one
    /// apart; inputs within `form`'s radius of a threshold are left to the
    /// exact evaluation.
    Split { form: Affine, at: f64, count: usize },
    /// The evaluation includes a step without an enclosure.
    Unsupported,
}

/// Grid positions whose values lie this close to a cell boundary take
/// both cells, joined, instead of a split (in grid units).
const FLAT: f64 = 1.0 / (1_u64 << 32) as f64;

impl Affine {
    /// The value `v` for every input.
    pub(crate) const fn point(v: f64) -> Self {
        Self {
            c: v,
            s: 0.0,
            r: 0.0,
            lo: v,
            hi: v,
        }
    }

    /// The inputs `low..=high` themselves.
    pub(crate) fn input(low: f64, high: f64) -> Self {
        let s = 0.5 * (high - low);
        let c = low + s;
        // `c ± s` misses `low` or `high` by at most the rounding of `c` and
        // of `high − low`.
        Self::new(c, s, grow(EPS * (c.abs() + s)), [low, high])
    }

    /// A form, exact where its bounds meet.
    fn new(c: f64, s: f64, r: f64, [lo, hi]: [f64; 2]) -> Self {
        if lo.to_bits() == hi.to_bits() {
            return Self::point(lo);
        }
        Self { c, s, r, lo, hi }
    }

    /// The value of every input, where the form is exact.
    pub(crate) fn value(self) -> Option<f64> {
        (self.s == 0.0 && self.r == 0.0).then_some(self.c)
    }

    /// The largest value `|c| + |s| + r` may stand for.
    fn magnitude(self) -> f64 {
        self.c.abs() + self.s.abs() + self.r
    }

    /// A lower bound of every value.
    pub(crate) fn low(self) -> f64 {
        (self.c - self.s.abs() - self.r - EPS * self.magnitude()).max(self.lo)
    }

    /// An upper bound of every value.
    pub(crate) fn high(self) -> f64 {
        (self.c + self.s.abs() + self.r + EPS * self.magnitude()).min(self.hi)
    }

    /// Whether every coefficient is finite.
    pub(crate) fn finite(self) -> bool {
        self.c.is_finite() && self.s.is_finite() && self.r.is_finite()
    }

    /// The form of one rounded operation whose exact result is
    /// `c + s·ξ ± r` (before `c` and `s` were rounded), with `bounds`.
    fn rounded(c: f64, s: f64, r: f64, bounds: [f64; 2]) -> Self {
        let magnitude = c.abs() + s.abs() + r;
        Self::new(c, s, grow(r + EPS * magnitude), bounds)
    }

    /// `x + y`. Exact values combine as the evaluation combines them: the
    /// same rounded operation on the same operands.
    pub(crate) fn add(self, y: Self) -> Self {
        if let (Some(a), Some(b)) = (self.value(), y.value()) {
            return Self::point(a + b);
        }
        let bounds = [self.lo + y.lo, self.hi + y.hi];
        Self::rounded(self.c + y.c, self.s + y.s, self.r + y.r, bounds)
    }

    /// `x − y`.
    pub(crate) fn sub(self, y: Self) -> Self {
        if let (Some(a), Some(b)) = (self.value(), y.value()) {
            return Self::point(a - b);
        }
        let bounds = [self.lo - y.hi, self.hi - y.lo];
        Self::rounded(self.c - y.c, self.s - y.s, self.r + y.r, bounds)
    }

    /// `x · k` for a constant `k`.
    pub(crate) fn scale(self, k: f64) -> Self {
        if let Some(a) = self.value() {
            return Self::point(a * k);
        }
        Self::rounded(
            self.c * k,
            self.s * k,
            self.r * k.abs(),
            ordered(self.lo * k, self.hi * k),
        )
    }

    /// `x / k` for a constant `k`.
    pub(crate) fn divide(self, k: f64) -> Self {
        if let Some(a) = self.value() {
            return Self::point(a / k);
        }
        Self::rounded(
            self.c / k,
            self.s / k,
            self.r / k.abs(),
            ordered(self.lo / k, self.hi / k),
        )
    }

    /// An enclosure of every value of `self` and of `other`.
    pub(crate) fn join(self, other: Self) -> Self {
        let c = f64::midpoint(self.c, other.c);
        let s = f64::midpoint(self.s, other.s);
        let reach = |x: Self| (x.c - c).abs() + (x.s - s).abs() + x.r;
        let r = grow(reach(self).max(reach(other)) + EPS * (c.abs() + s.abs()));
        let [lo, _] = ordered(self.lo, other.lo);
        let [_, hi] = ordered(self.hi, other.hi);
        Self::new(c, s, r, [lo, hi])
    }

    /// `x.clamp(low, high)` (`f64::clamp`). Clamping moves no value
    /// farther from another, so where the line `c + s·ξ` stays within
    /// `[low, high]`, or leaves it by less than the radius, the clamped
    /// value stays as close to the line (or to the bound) as the value was.
    pub(crate) fn clamp(self, low: f64, high: f64) -> Result<Self, Undecided> {
        let (bottom, top) = (self.low(), self.high());
        if bottom >= low && top <= high {
            return Ok(self);
        }
        // A value equal to a bound stays as it is, which may be the other
        // zero.
        if top < low {
            return Ok(Self::point(low));
        }
        if bottom > high {
            return Ok(Self::point(high));
        }
        let bounds = clamp_bounds([self.lo, self.hi], low, high);
        let slack = EPS * self.magnitude();
        let (from, to) = (self.c - self.s.abs(), self.c + self.s.abs());
        let outside = (low - from).max(to - high).max(0.0);
        if outside <= 4.0 * self.r + slack {
            return Ok(Self::new(
                self.c,
                self.s,
                grow(self.r + outside + slack),
                bounds,
            ));
        }
        if to - low <= 4.0 * self.r + slack {
            // Every value is at most `top`: clamped, within `[low, top]`.
            return Ok(Self::new(low, 0.0, grow(top - low), bounds));
        }
        if high - from <= 4.0 * self.r + slack {
            return Ok(Self::new(high, 0.0, grow(high - bottom), bounds));
        }
        // Inputs near the crossing are left out, so that the parts clamp
        // all or none of their values, exactly.
        Err(Undecided::Split {
            form: self,
            at: if from < low { low } else { high },
            count: 1,
        })
    }

    /// Whether every value is at least `bound` (`x >= bound`).
    pub(crate) fn at_least(self, bound: f64) -> Result<bool, Undecided> {
        if self.low() >= bound {
            Ok(true)
        } else if self.high() < bound {
            Ok(false)
        } else {
            Err(Undecided::Split {
                form: self,
                at: bound,
                count: 1,
            })
        }
    }

    /// The grid cells `k` (`floor(x)` clipped to `last`) the values take:
    /// one cell, or two adjacent cells when the values lie within [`FLAT`]
    /// of their common boundary; the caller joins the enclosures of what
    /// each cell computes. Otherwise every boundary the values cross splits
    /// them. Piecewise-linear interpolation is continuous
    /// across a boundary, so both cells compute nearly the same value there.
    pub(crate) fn cells(self, last: usize) -> Result<[usize; 2], Undecided> {
        // Grid positions are clipped to the grid, so no value is negative.
        let (bottom, top) = (self.low().max(0.0), self.high());
        if !(top >= 0.0 && top.is_finite()) {
            return Err(Undecided::Unsupported);
        }
        let first = index(bottom).min(last);
        let end = index(top).min(last);
        if first == end {
            return Ok([first, end]);
        }
        if end == first + 1 && top - bottom <= FLAT {
            return Ok([first, end]);
        }
        // Every boundary crossed at once: the cells between them are the
        // parts, without an enclosure for each boundary.
        Err(Undecided::Split {
            form: self,
            at: usize_f64(first + 1),
            count: end - first,
        })
    }
}

/// `[a, b]` in IEEE total order.
fn ordered(a: f64, b: f64) -> [f64; 2] {
    if a.total_cmp(&b).is_le() {
        [a, b]
    } else {
        [b, a]
    }
}

/// Bounds of `x.clamp(low, high)` for every `x` within `[lo, hi]`: the
/// values that stay (real values in `[low, high]`, either zero at a zero
/// bound) and the bounds that replace the rest.
fn clamp_bounds([lo, hi]: [f64; 2], low: f64, high: f64) -> [f64; 2] {
    let floor = if low == 0.0 { -0.0 } else { low };
    let ceiling = if high == 0.0 { 0.0 } else { high };
    let [mut bottom, mut top] = [f64::INFINITY, f64::NEG_INFINITY];
    let mut take = |v: f64| {
        bottom = ordered(bottom, v)[0];
        top = ordered(top, v)[1];
    };
    let stays = [ordered(lo, floor)[1], ordered(hi, ceiling)[0]];
    if stays[0].total_cmp(&stays[1]).is_le() {
        take(stays[0]);
        take(stays[1]);
    }
    if lo < low {
        take(low);
    }
    if hi > high {
        take(high);
    }
    [bottom, top]
}

/// A radius computed in rounded arithmetic, raised past the rounding of the
/// few operations that computed it and past the absolute error of a
/// subnormal result.
pub(crate) fn grow(r: f64) -> f64 {
    r * (1.0 + 4.0 * EPS) + f64::MIN_POSITIVE
}

/// `x as usize`, which truncates as the evaluations' grid indices do.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "grid positions are finite and nonnegative"
)]
fn index(x: f64) -> usize {
    x as usize
}

#[expect(
    clippy::cast_precision_loss,
    reason = "grid and bucket counts come from u32 values, which f64 holds exactly"
)]
pub(crate) fn usize_f64(n: usize) -> f64 {
    n as f64
}
