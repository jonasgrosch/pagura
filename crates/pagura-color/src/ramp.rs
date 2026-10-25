//! Certified colour ramps: the colours of a one-input shading stage
//! (functions, then a conversion) found without evaluating them, where only
//! a rounding of each value is kept.
//!
//! The function input interval is cut into pieces on which every discrete
//! choice of the exact evaluation (subdomain, grid cell, clipping, the
//! transfer function's branch) is the same. On a piece, affine arithmetic
//! ([`crate::affine`]) encloses every value the exact evaluation computes
//! within a radius of a line `a + b·x`, rounding error included. A sample
//! then takes `a + b·x` and keeps it where both ends of its enclosure round
//! alike: the exact value lies between them, so it rounds alike too. An sRGB
//! channel above the linear segment takes a cubic within a proven radius of
//! its encoded value ([`cie::power_cubic`]), or where no cubic is close
//! enough, encloses the linear value and lets the bounds of its power
//! ([`cie::power_bounds`]) decide the rounding. A sample whose ends round
//! apart, or that no piece holds, is the caller's to evaluate exactly.

use crate::affine::{Affine, Undecided, grow, usize_f64};
use crate::cie;
use crate::convert::{Channel, Converter};
use crate::function::Function;
use crate::space::MAX_COMPONENTS;
use std::sync::Arc;

/// The most channels a ramp converts into.
const MAX_RAMP_CHANNELS: usize = 4;

/// The widest radius an encoded channel's cubic may have; a wider one
/// leaves more samples undecided than bounding the power per sample.
const CUBIC_RADIUS: f64 = 1.0 / (1_u64 << 36) as f64;

/// The colour stage a ramp encloses: one function giving every colour
/// component, or one function per component, each of one input.
#[derive(Debug, Clone, Copy)]
pub enum RampStage<'a> {
    /// One function of every component.
    Single(&'a Function),
    /// One function per component.
    PerComponent(&'a [Arc<Function>]),
}

/// Pieces of a function input interval with the lines that enclose every
/// converted value on them.
#[derive(Debug, Clone, Default)]
pub struct Ramp {
    channels: usize,
    /// Enclosures computed while cutting.
    enclosures: usize,
    /// Ordered, disjoint input intervals.
    pieces: Vec<[f64; 2]>,
    /// `channels` terms per piece.
    terms: Vec<Term>,
    /// For each bucket of inputs (see [`Self::bucket`]), the first piece
    /// that may hold an input in it, or `u32::MAX`.
    index: Vec<u32>,
    /// Buckets are `⌊(x − origin)·scale⌋`, clipped to the last.
    origin: f64,
    scale: f64,
}

/// One channel on one piece: within `radius` of the cubic `k₀ + k₁·τ +
/// k₂·τ² + k₃·τ³` in `τ = x − centre`, evaluated by Horner's rule (a line
/// `a + b·x` has `k = [a, b, 0, 0]` and centre 0, which evaluates as
/// `a + b·x`); exactly `k₀` where `radius` is zero; or for an sRGB channel
/// whose power is bounded per sample, the linear value within `radius` of
/// the line and its power's bounds wide by `margin`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Term {
    k: [f64; 4],
    centre: f64,
    radius: f64,
    /// Zero but for a power bounded per sample.
    margin: f64,
}

impl Ramp {
    /// Bytes per piece of a ramp converting into `channels` channels, its
    /// share of the index (at most four buckets) included.
    pub fn piece_bytes(channels: usize) -> usize {
        size_of::<[f64; 2]>() + channels * size_of::<Term>() + 4 * size_of::<u32>()
    }

    /// Cuts `inputs` (bounds in IEEE total order) into at most `limit`
    /// pieces for `stage`, whose `components` colour components `converter`
    /// converts, after at most `4 · limit` enclosures. The ramp is empty
    /// where the stage or conversion has no enclosure or converts into more
    /// than four channels. Enclosures exist for exponential functions with
    /// `N` 1, sampled functions of one input and order 1, stitching
    /// functions of them, and conversions through range clips and
    /// one-input tint transforms into the target's own family, from device
    /// gray, RGB or CMYK, or from an ICC matrix/TRC profile with identity
    /// or sampled curves. Inputs no piece holds are left to exact
    /// evaluation.
    pub fn new(
        stage: RampStage<'_>,
        components: usize,
        converter: &Converter,
        inputs: [f64; 2],
        limit: usize,
    ) -> Self {
        let channels = converter.target().channels();
        let one_input = match stage {
            RampStage::Single(f) => f.inputs() == 1,
            RampStage::PerComponent(fs) => fs.iter().all(|f| f.inputs() == 1),
        };
        if !(one_input
            && (1..=MAX_COMPONENTS).contains(&components)
            && (1..=MAX_RAMP_CHANNELS).contains(&channels)
            && inputs[0].total_cmp(&inputs[1]).is_le()
            && inputs.iter().all(|v| v.is_finite()))
        {
            return Self::default();
        }
        let enclose = |x: Affine, out: &mut [Channel]| {
            let mut values = [Affine::point(0.0); MAX_COMPONENTS];
            let values = &mut values[..components];
            match stage {
                RampStage::Single(f) => f.enclose(x, values)?,
                RampStage::PerComponent(fs) => {
                    for (f, value) in fs.iter().zip(values.iter_mut()) {
                        f.enclose(x, std::slice::from_mut(value))?;
                    }
                }
            }
            converter.enclose(values, out)
        };
        let mut ramp = Self {
            channels,
            ..Self::default()
        };
        if ramp.pieces.try_reserve_exact(limit).is_err()
            || ramp
                .terms
                .try_reserve_exact(limit.saturating_mul(channels))
                .is_err()
        {
            return Self::default();
        }
        let mut out = [Channel::Value(Affine::point(0.0)); MAX_RAMP_CHANNELS];
        let out = &mut out[..channels];
        // Intervals to enclose, the leftmost last: pieces come out ordered.
        let mut pending = vec![inputs];
        let mut budget = limit.saturating_mul(4);
        while let Some([low, high]) = pending.pop() {
            if budget == 0 || ramp.pieces.len() == limit {
                break;
            }
            budget -= 1;
            ramp.enclosures += 1;
            let x = Affine::input(low, high);
            match enclose(x, out) {
                Ok(()) => ramp.push([low, high], x, out),
                // Over the whole interval: the stage itself has no enclosure.
                Err(Undecided::Unsupported) if [low, high] == inputs => break,
                Err(Undecided::Unsupported) => {}
                Err(Undecided::Split { form, at }) => {
                    let [left, right] = split([low, high], x, form, at);
                    pending.extend(right);
                    pending.extend(left);
                }
            }
        }
        ramp.pieces.shrink_to_fit();
        ramp.terms.shrink_to_fit();
        if !ramp.index(inputs) {
            return Self::default();
        }
        ramp
    }

    /// Builds the bucket index over `inputs`, two buckets or more a piece;
    /// false where it cannot be held.
    fn index(&mut self, [low, high]: [f64; 2]) -> bool {
        let buckets = (2 * self.pieces.len()).next_power_of_two();
        if self.index.try_reserve_exact(buckets).is_err() {
            return false;
        }
        self.index.resize(buckets, u32::MAX);
        self.origin = low;
        self.scale = usize_f64(buckets) / (high - low);
        if !self.scale.is_finite() {
            self.scale = 0.0;
        }
        // Bucketing is nondecreasing, so an input of a piece falls in a
        // bucket between those of the piece's ends; the last piece to
        // claim a bucket, walking back, is the first that may hold it.
        for i in (0..self.pieces.len()).rev() {
            let [low, high] = self.pieces[i];
            let (first, last) = (self.bucket(low), self.bucket(high));
            let piece = u32::try_from(i).unwrap_or(u32::MAX);
            self.index[first..=last].fill(piece);
        }
        true
    }

    /// The bucket of `x`: nondecreasing in `x`, and 0 for NaN.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a saturating cast is the bucket, clipped below"
    )]
    fn bucket(&self, x: f64) -> usize {
        (((x - self.origin) * self.scale) as usize).min(self.index.len().saturating_sub(1))
    }

    /// Enclosures computed while cutting, each about as costly as a few
    /// exact evaluations.
    pub fn enclosures(&self) -> usize {
        self.enclosures
    }

    /// Bytes held.
    pub fn bytes(&self) -> usize {
        self.pieces.capacity() * size_of::<[f64; 2]>()
            + self.terms.capacity() * size_of::<Term>()
            + self.index.capacity() * size_of::<u32>()
    }

    /// Pieces held.
    pub fn len(&self) -> usize {
        self.pieces.len()
    }

    /// Whether no piece is held.
    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    /// Writes into `output` (the target's channels) values that each round
    /// by `round` (nondecreasing) as the exact conversion of the stage at
    /// `x` does, and returns true; false where a piece does not decide them,
    /// leaving `output` to the caller.
    #[inline]
    pub fn convert(&self, x: f64, output: &mut [f64], round: impl Fn(f64) -> u32) -> bool {
        let Some(piece) = self.find(x) else {
            return false;
        };
        let start = piece * self.channels;
        let Some(terms) = self.terms.get(start..start + self.channels) else {
            return false;
        };
        for (out, term) in output.iter_mut().zip(terms) {
            let [k0, k1, k2, k3] = term.k;
            if term.radius == 0.0 {
                // The value itself, a zero's sign included.
                *out = k0;
                continue;
            }
            let t = x - term.centre;
            let v = k0 + t * (k1 + t * (k2 + t * k3));
            *out = if term.margin == 0.0 {
                // Rounding tells the zeros apart, so a zero must not be
                // among the values.
                let (low, high) = (v - term.radius, v + term.radius);
                if !(low > 0.0 || high < 0.0) || round(low) != round(high) {
                    return false;
                }
                v
            } else {
                match cie::power_bounds(v, term.margin) {
                    Some([low, high]) if round(low) == round(high) => low,
                    _ => return false,
                }
            };
        }
        true
    }

    /// The piece holding `x`. Pieces are ordered, so the first piece from
    /// the bucket's on whose end reaches `x` is the only one that can.
    #[inline]
    fn find(&self, x: f64) -> Option<usize> {
        let mut i = usize::try_from(*self.index.get(self.bucket(x))?).ok()?;
        while let Some(&[_, high]) = self.pieces.get(i)
            && high < x
        {
            i += 1;
        }
        let &[low, high] = self.pieces.get(i)?;
        // In total order: a piece from `+0` does not hold `−0`.
        let holds = low <= x
            && x <= high
            && (x != 0.0 || (low.total_cmp(&x).is_le() && x.total_cmp(&high).is_le()));
        holds.then_some(i)
    }

    /// Adds the piece `low..=high`, whose input form is `x`, with the
    /// enclosures `out`; a piece some channel of which cannot be kept is
    /// left out.
    fn push(&mut self, [low, high]: [f64; 2], x: Affine, out: &[Channel]) {
        let mut terms = [Term {
            k: [0.0; 4],
            centre: 0.0,
            radius: 0.0,
            margin: 0.0,
        }; MAX_RAMP_CHANNELS];
        for (term, &channel) in terms.iter_mut().zip(out) {
            let Some(t) = Term::new(channel, x, [low, high]) else {
                return;
            };
            *term = t;
        }
        self.pieces.push([low, high]);
        self.terms.extend_from_slice(&terms[..out.len()]);
    }
}

impl Term {
    /// The term of `channel` enclosed over the input form `x` of the inputs
    /// `low..=high`.
    ///
    /// For an input `t`, every form holds the value at the same `ξ`, which
    /// is `(t − x.c) / x.s` up to `x.r / x.s`. The line `a + b·t` with
    /// `b = s / x.s` and `a = c − b·x.c` (each rounded) differs from
    /// `c + s·ξ` by at most `|s| · x.r / x.s` and the rounding of `a` and
    /// `b`; evaluating it rounds twice more, and so do its ends `v ∓ radius`.
    fn new(channel: Channel, x: Affine, [low, high]: [f64; 2]) -> Option<Self> {
        let reach = low.abs().max(high.abs());
        let (form, encoded) = match channel {
            Channel::Value(form) => (form, false),
            Channel::Encoded(form) => (form, true),
        };
        if !form.finite() {
            return None;
        }
        if let Some(value) = form.value() {
            // One exact value: its conversion is the same for every input.
            let value = if encoded { cie::encode(value) } else { value };
            return Some(Self {
                k: [value, 0.0, 0.0, 0.0],
                centre: 0.0,
                radius: 0.0,
                margin: 0.0,
            });
        }
        let (b, a, offset) = if x.s > 0.0 {
            let b = form.s / x.s;
            (b, form.c - b * x.c, form.s.abs() * (x.r / x.s))
        } else {
            (0.0, form.c, 0.0)
        };
        let rounding =
            form.c.abs() + (b * x.c).abs() + form.s.abs() + 2.0 * (a.abs() + (b * reach).abs());
        let radius = grow(form.r + offset + f64::EPSILON * rounding);
        if !radius.is_finite() {
            return None;
        }
        let line = Self {
            k: [a, b, 0.0, 0.0],
            centre: 0.0,
            radius,
            margin: 0.0,
        };
        if !encoded {
            return Some(line);
        }
        // Every linear value is at least `low`, so every sample's `v` at
        // least `low − radius`; `|w^(5/12) / v^(5/12) − 1| ≤ (5/12) · radius /
        // (v − radius)` for a value `w` within `radius` of `v`.
        let room = form.low() - 2.0 * radius;
        if let Some((k, centre, radius)) = cie::power_cubic([a, b], radius, room, [low, high])
            && radius <= CUBIC_RADIUS
        {
            return Some(Self {
                k,
                centre,
                radius,
                margin: 0.0,
            });
        }
        let spread = 0.42 * radius / room;
        if !(room > 0.0 && spread <= 1.0 / (1_u64 << 10) as f64) {
            return None;
        }
        Some(Self {
            margin: grow((cie::MARGIN + spread) * (1.0 + 1.0 / (1_u64 << 40) as f64)),
            ..line
        })
    }
}

/// The parts of `low..=high` (whose input form is `x`) on either side of
/// where `form` reaches `at`, leaving out the inputs within twice `form`'s
/// radius of the threshold, whose choice the enclosure cannot tell. A part
/// that would not shrink the interval is dropped.
fn split([low, high]: [f64; 2], x: Affine, form: Affine, at: f64) -> [Option<[f64; 2]>; 2] {
    if form.s == 0.0 || x.s == 0.0 {
        return [None, None];
    }
    let centre = (at - form.c) / form.s;
    let width = 2.0 * (form.r + f64::EPSILON * (form.c.abs() + form.s.abs() + form.r))
        / form.s.abs()
        + 1.0 / (1_u64 << 40) as f64;
    // The inputs at `ξ` rise with it.
    let input = |xi: f64| x.c + x.s * xi;
    let (first, second) = (input(centre - width), input(centre + width));
    let left_end = first.next_down().min(high.next_down());
    let right_start = left_end.next_up().max(second.next_up()).max(low.next_up());
    let left = (left_end >= low && left_end < high).then_some([low, left_end]);
    let right = (right_start <= high && right_start > low).then_some([right_start, high]);
    [left, right]
}

#[cfg(test)]
mod tests;
