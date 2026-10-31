//! Double-word arithmetic for the frames of true curves.
//!
//! A value is the unevaluated sum `hi + lo` of two doubles with `lo` at most
//! half an ulp of `hi`. The algorithms and their relative error bounds, in
//! units of `u = ε/2`, are those of Joldes, Muller and Popescu, "Tight and
//! rigorous error bounds for basic building blocks of double-word
//! arithmetic" (ACM TOMS 44, 2017):
//!
//! - sums of two doubles and products of two doubles are exact (2Sum,
//!   2Prod by a fused multiply-add);
//! - a sum of two double-words errs by at most `3u²` of the exact sum
//!   (AccurateDWPlusDW);
//! - a product by a double by at most `2u²` (DWTimesFP3);
//! - a product of two double-words by at most `5u²` (DWTimesDW3);
//! - halving is exact.
//!
//! The bounds assume no underflow; a result below `f64::MIN_POSITIVE` adds
//! at most that much absolute error, which the curve bound also carries.

use pagura_raster::{Point, Transform};

/// A double-word: `hi + lo`, with `|lo|` at most half an ulp of `hi`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Double {
    pub(crate) hi: f64,
    pub(crate) lo: f64,
}

impl From<f64> for Double {
    fn from(hi: f64) -> Self {
        Self { hi, lo: 0.0 }
    }
}

/// The exact sum of two doubles.
pub(crate) fn two_sum(a: f64, b: f64) -> Double {
    let hi = a + b;
    let b_virtual = hi - a;
    Double {
        hi,
        lo: (a - (hi - b_virtual)) + (b - b_virtual),
    }
}

/// The exact sum of two doubles, `|a| >= |b|` or `a` zero.
fn fast_two_sum(a: f64, b: f64) -> Double {
    let hi = a + b;
    Double {
        hi,
        lo: b - (hi - a),
    }
}

/// The exact product of two doubles.
pub(crate) fn two_product(a: f64, b: f64) -> Double {
    let hi = a * b;
    Double {
        hi,
        lo: a.mul_add(b, -hi),
    }
}

impl Double {
    pub(crate) const ZERO: Self = Self { hi: 0.0, lo: 0.0 };

    /// 2/3, within `(2/3)·u²`: the residual `2 − 3·hi` is exact.
    pub(crate) fn two_thirds() -> Self {
        let hi = 2.0 / 3.0;
        Self {
            hi,
            lo: (-3.0_f64).mul_add(hi, 2.0) / 3.0,
        }
    }

    pub(crate) fn add(self, other: Self) -> Self {
        let s = two_sum(self.hi, other.hi);
        let t = two_sum(self.lo, other.lo);
        let v = fast_two_sum(s.hi, s.lo + t.hi);
        fast_two_sum(v.hi, t.lo + v.lo)
    }

    pub(crate) fn neg(self) -> Self {
        Self {
            hi: -self.hi,
            lo: -self.lo,
        }
    }

    pub(crate) fn times(self, b: f64) -> Self {
        let c = two_product(self.hi, b);
        fast_two_sum(c.hi, self.lo.mul_add(b, c.lo))
    }

    pub(crate) fn mul(self, other: Self) -> Self {
        let c = two_product(self.hi, other.hi);
        let t = self.hi.mul_add(other.lo, self.lo * other.lo);
        let low = self.lo.mul_add(other.hi, t);
        fast_two_sum(c.hi, c.lo + low)
    }

    pub(crate) fn half(self) -> Self {
        Self {
            hi: self.hi * 0.5,
            lo: self.lo * 0.5,
        }
    }

    /// The nearest double.
    pub(crate) fn value(self) -> f64 {
        self.hi + self.lo
    }
}

/// A point of double-words.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DoublePoint {
    pub(crate) x: Double,
    pub(crate) y: Double,
}

impl DoublePoint {
    pub(crate) const ZERO: Self = Self {
        x: Double::ZERO,
        y: Double::ZERO,
    };

    /// The exact difference `p − q`.
    pub(crate) fn difference(p: Point, q: Point) -> Self {
        Self {
            x: two_sum(p.x, -q.x),
            y: two_sum(p.y, -q.y),
        }
    }

    /// `p` placed by `transform`: two exact products and two sums.
    pub(crate) fn placed(p: Point, transform: Transform) -> Self {
        let t = transform;
        Self {
            x: two_product(p.x, t.a)
                .add(two_product(p.y, t.c))
                .add(Double::from(t.e)),
            y: two_product(p.x, t.b)
                .add(two_product(p.y, t.d))
                .add(Double::from(t.f)),
        }
    }

    pub(crate) fn add(self, other: Self) -> Self {
        Self {
            x: self.x.add(other.x),
            y: self.y.add(other.y),
        }
    }

    pub(crate) fn sub(self, other: Self) -> Self {
        Self {
            x: self.x.add(other.x.neg()),
            y: self.y.add(other.y.neg()),
        }
    }

    pub(crate) fn scale(self, k: Double) -> Self {
        Self {
            x: self.x.mul(k),
            y: self.y.mul(k),
        }
    }

    pub(crate) fn midpoint(self, other: Self) -> Self {
        Self {
            x: self.x.add(other.x).half(),
            y: self.y.add(other.y).half(),
        }
    }

    /// This point under the linear part of `transform`.
    pub(crate) fn mapped(self, transform: Transform) -> Self {
        let t = transform;
        Self {
            x: self.x.times(t.a).add(self.y.times(t.c)),
            y: self.x.times(t.b).add(self.y.times(t.d)),
        }
    }

    /// The nearest point of doubles.
    pub(crate) fn point(self) -> Point {
        Point::new(self.x.value(), self.y.value())
    }
}

/// Exact rationals `n / (3^k · 2^e)` over a small signed big integer, to
/// test the double-word frames against exact arithmetic.
#[cfg(test)]
pub(crate) mod rational {
    use std::cmp::Ordering;

    /// A signed integer of 32-bit limbs, least significant first.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Big {
        negative: bool,
        limbs: Vec<u32>,
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "limb arithmetic keeps every pushed value within 32 bits"
    )]
    impl Big {
        fn zero() -> Self {
            Self {
                negative: false,
                limbs: Vec::new(),
            }
        }

        fn from_u64(value: u64, negative: bool) -> Self {
            let mut big = Self {
                negative,
                limbs: vec![(value & 0xffff_ffff) as u32, (value >> 32) as u32],
            };
            big.trim();
            big
        }

        fn trim(&mut self) {
            while self.limbs.last() == Some(&0) {
                self.limbs.pop();
            }
            if self.limbs.is_empty() {
                self.negative = false;
            }
        }

        fn compare_magnitude(a: &[u32], b: &[u32]) -> Ordering {
            a.len()
                .cmp(&b.len())
                .then_with(|| a.iter().rev().cmp(b.iter().rev()))
        }

        fn add_magnitude(a: &[u32], b: &[u32]) -> Vec<u32> {
            let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
            let mut carry = 0_u64;
            for i in 0..a.len().max(b.len()) {
                let sum =
                    u64::from(*a.get(i).unwrap_or(&0)) + u64::from(*b.get(i).unwrap_or(&0)) + carry;
                out.push((sum & 0xffff_ffff) as u32);
                carry = sum >> 32;
            }
            out.push(carry as u32);
            out
        }

        /// `a − b` for `|a| >= |b|`.
        fn sub_magnitude(a: &[u32], b: &[u32]) -> Vec<u32> {
            let mut out = Vec::with_capacity(a.len());
            let mut borrow = 0_i64;
            for (i, &limb) in a.iter().enumerate() {
                let mut difference = i64::from(limb) - i64::from(*b.get(i).unwrap_or(&0)) - borrow;
                borrow = i64::from(difference < 0);
                difference += borrow << 32;
                out.push(difference as u32);
            }
            out
        }

        fn add(&self, other: &Self) -> Self {
            let mut out = if self.negative == other.negative {
                Self {
                    negative: self.negative,
                    limbs: Self::add_magnitude(&self.limbs, &other.limbs),
                }
            } else if Self::compare_magnitude(&self.limbs, &other.limbs) == Ordering::Less {
                Self {
                    negative: other.negative,
                    limbs: Self::sub_magnitude(&other.limbs, &self.limbs),
                }
            } else {
                Self {
                    negative: self.negative,
                    limbs: Self::sub_magnitude(&self.limbs, &other.limbs),
                }
            };
            out.trim();
            out
        }

        fn neg(&self) -> Self {
            let mut out = self.clone();
            out.negative = !out.negative;
            out.trim();
            out
        }

        fn mul_small(&self, factor: u64) -> Self {
            let (low, high) = (factor & 0xffff_ffff, factor >> 32);
            let times = |f: u64| {
                let mut out = Vec::with_capacity(self.limbs.len() + 1);
                let mut carry = 0_u64;
                for &limb in &self.limbs {
                    let product = u64::from(limb) * f + carry;
                    out.push((product & 0xffff_ffff) as u32);
                    carry = product >> 32;
                }
                out.push(carry as u32);
                out
            };
            let mut shifted = vec![0];
            shifted.extend(times(high));
            let mut out = Self {
                negative: self.negative,
                limbs: Self::add_magnitude(&times(low), &shifted),
            };
            out.trim();
            out
        }

        fn shl(&self, bits: u32) -> Self {
            let mut limbs = vec![0; (bits / 32) as usize];
            let shift = bits % 32;
            let mut carry = 0_u32;
            for &limb in &self.limbs {
                limbs.push(if shift == 0 {
                    limb
                } else {
                    (limb << shift) | carry
                });
                carry = if shift == 0 { 0 } else { limb >> (32 - shift) };
            }
            limbs.push(carry);
            let mut out = Self {
                negative: self.negative,
                limbs,
            };
            out.trim();
            out
        }
    }

    /// `numerator / (3^threes · 2^twos)`.
    #[derive(Debug, Clone)]
    pub(crate) struct Rational {
        numerator: Big,
        threes: u32,
        twos: i32,
    }

    impl Rational {
        /// A double, exactly.
        pub(crate) fn from_f64(value: f64) -> Self {
            assert!(value.is_finite());
            if value == 0.0 {
                return Self {
                    numerator: Big::zero(),
                    threes: 0,
                    twos: 0,
                };
            }
            let bits = value.to_bits();
            let exponent = ((bits >> 52) & 0x7ff) as i32;
            let fraction = bits & ((1 << 52) - 1);
            let (mantissa, exponent) = if exponent == 0 {
                (fraction, -1074)
            } else {
                (fraction | (1 << 52), exponent - 1075)
            };
            Self {
                numerator: Big::from_u64(mantissa, value < 0.0),
                threes: 0,
                twos: -exponent,
            }
        }

        /// Both over the same denominator.
        fn aligned(&self, other: &Self) -> (Big, Big, u32, i32) {
            let threes = self.threes.max(other.threes);
            let twos = self.twos.max(other.twos);
            let lift = |r: &Self| {
                let mut n = r.numerator.clone();
                for _ in r.threes..threes {
                    n = n.mul_small(3);
                }
                n.shl(u32::try_from(twos - r.twos).unwrap())
            };
            (lift(self), lift(other), threes, twos)
        }

        pub(crate) fn add(&self, other: &Self) -> Self {
            let (a, b, threes, twos) = self.aligned(other);
            Self {
                numerator: a.add(&b),
                threes,
                twos,
            }
        }

        pub(crate) fn sub(&self, other: &Self) -> Self {
            self.add(&other.neg())
        }

        pub(crate) fn neg(&self) -> Self {
            Self {
                numerator: self.numerator.neg(),
                ..self.clone()
            }
        }

        /// This times a double, exactly.
        pub(crate) fn times(&self, value: f64) -> Self {
            let factor = Self::from_f64(value);
            let magnitude = factor
                .numerator
                .limbs
                .iter()
                .rev()
                .fold(0_u64, |m, &l| (m << 32) | u64::from(l));
            let mut numerator = self.numerator.mul_small(magnitude);
            numerator.negative = numerator.negative != factor.numerator.negative;
            numerator.trim();
            Self {
                numerator,
                threes: self.threes,
                twos: self.twos + factor.twos,
            }
        }

        pub(crate) fn half(&self) -> Self {
            Self {
                twos: self.twos + 1,
                ..self.clone()
            }
        }

        /// Two thirds of this, exactly.
        pub(crate) fn two_thirds(&self) -> Self {
            Self {
                numerator: self.numerator.shl(1),
                threes: self.threes + 1,
                twos: self.twos,
            }
        }

        /// Whether `|self − value| <= bound`, decided exactly.
        pub(crate) fn within(&self, value: f64, bound: f64) -> bool {
            let difference = self.sub(&Self::from_f64(value));
            let (d, b, _, _) = difference.aligned(&Self::from_f64(bound.abs()));
            Big::compare_magnitude(&d.limbs, &b.limbs) != Ordering::Greater
        }

        /// Whether `|self − (hi + lo)| <= bound`, decided exactly.
        pub(crate) fn within_double(&self, hi: f64, lo: f64, bound: f64) -> bool {
            self.sub(&Self::from_f64(lo)).within(hi, bound)
        }
    }

    #[test]
    fn rationals_are_exact() {
        let third = Rational::from_f64(1.0).two_thirds().half();
        let sum = third.add(&third).add(&third);
        assert!(sum.within(1.0, 0.0));
        let tiny = Rational::from_f64(f64::MIN_POSITIVE / 8.0).times(3.0);
        assert!(tiny.within(3.0 * f64::MIN_POSITIVE / 8.0, 0.0));
        let x = Rational::from_f64(1e300).times(-1e-300);
        assert!(x.within(-1.0, 1e-15));
        let product = super::two_product(1e300, -1e-300);
        assert!(x.within_double(product.hi, product.lo, 0.0));
        assert!(!x.within(product.hi, 0.0) || product.lo == 0.0);
    }
}
