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
use pagura_core::Meter;
use std::sync::Arc;

/// The most channels a ramp converts into.
pub const MAX_RAMP_CHANNELS: usize = 4;

/// The widest radius an encoded channel's cubic may have; a wider one
/// leaves more samples undecided than bounding the power per sample.
const CUBIC_RADIUS: f64 = 1.0 / (1_u64 << 36) as f64;

/// How far a [`Piece`] may write a value from the exact one.
pub const APPROXIMATE: f64 = 1.0 / (1_u64 << 30) as f64;

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

/// One piece of a [`Ramp`] for inputs known within an error
/// ([`Ramp::piece`]): for every input it holds, [`Self::evaluate`] writes
/// values within [`Self::radius`], at most [`APPROXIMATE`], of the exact
/// ones at any input within the error of it.
#[derive(Debug, Clone, Copy)]
pub struct Piece<'a> {
    /// The inputs held: the piece's, short of the error at both ends.
    low: f64,
    high: f64,
    terms: &'a [Term],
    /// Each channel's [`Self::radius`].
    radii: [f64; MAX_RAMP_CHANNELS],
}

impl Piece<'_> {
    /// Whether the piece holds `x`.
    #[inline]
    pub fn holds(&self, x: f64) -> bool {
        self.low <= x && x <= self.high
    }

    /// Writes the target's channels for `x`, an input the piece holds.
    #[inline]
    pub fn evaluate(&self, x: f64, output: &mut [f64]) {
        for (out, term) in output.iter_mut().zip(self.terms) {
            *out = horner(term.k, x - term.centre);
        }
    }

    /// [`Self::evaluate`] at each of `inputs`, every one held: channel `c`
    /// at `inputs[j]` into `output[c][j]`, bit for bit as [`Self::evaluate`]
    /// writes it. Channel by channel, so that each channel's run
    /// vectorizes.
    #[inline]
    pub fn evaluate_each<const N: usize>(
        &self,
        inputs: &[f64; N],
        output: &mut [[f64; N]; MAX_RAMP_CHANNELS],
    ) {
        for (out, term) in output.iter_mut().zip(self.terms) {
            for (out, &x) in out.iter_mut().zip(inputs) {
                *out = horner(term.k, x - term.centre);
            }
        }
    }

    /// How far channel `channel`'s evaluated value may lie from the exact
    /// one: zero for a constant term, which is exact, and otherwise the
    /// bound `2R + S·error` of [`Ramp::piece`], rounded up, at most
    /// [`APPROXIMATE`]; infinite past the channels.
    #[inline]
    pub fn radius(&self, channel: usize) -> f64 {
        self.radii.get(channel).copied().unwrap_or(f64::INFINITY)
    }
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
    /// `S`, a bound of the polynomial's slope on its piece
    /// ([`Self::slope_on`]), found as the piece is added ([`Ramp::push`]).
    slope: f64,
}

impl Ramp {
    /// Cuts `inputs` (bounds in IEEE total order) into at most `limit`
    /// pieces for `stage`, whose `components` colour components `converter`
    /// converts, after at most `4 · limit` enclosures. Its storage is
    /// charged to `bytes` before it is allocated, temporaries included
    /// (the intervals pending while it cuts, and the replacements of shrunk
    /// buffers), and temporaries are refunded once released, so `bytes`
    /// keeps [`Self::bytes`] charged. The ramp is empty, with every charge
    /// returned, where `bytes` lacks room at any point, where the stage or
    /// conversion has no enclosure, or where it converts into more than
    /// four channels. Enclosures exist for exponential functions with
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
        (inputs, limit): ([f64; 2], usize),
        bytes: &mut Meter,
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
        let used = bytes.used();
        let mut ramp = Self {
            channels,
            ..Self::default()
        };
        if ramp.cut(enclose, (inputs, limit), bytes).is_ok() {
            return ramp;
        }
        let enclosures = ramp.enclosures;
        drop(ramp);
        // Everything charged since has been released.
        bytes.refund(bytes.used().saturating_sub(used));
        // Released storage does not undo the work already spent cutting.
        Self {
            enclosures,
            ..Self::default()
        }
    }

    /// Cuts `inputs` into pieces by `enclose`, as [`Self::new`] does,
    /// charging storage to `bytes`.
    fn cut(
        &mut self,
        enclose: impl Fn(Affine, &mut [Channel]) -> Result<(), Undecided>,
        (inputs, limit): ([f64; 2], usize),
        bytes: &mut Meter,
    ) -> pagura_core::Result<()> {
        bytes.reserve(&mut self.pieces, limit)?;
        bytes.reserve(&mut self.terms, limit.saturating_mul(self.channels))?;
        let mut out = [Channel::Value(Affine::point(0.0)); MAX_RAMP_CHANNELS];
        let out = &mut out[..self.channels];
        // Intervals to enclose, the leftmost last: pieces come out ordered.
        let mut pending = Vec::new();
        bytes.reserve_capacity(&mut pending, 1)?;
        pending.push(inputs);
        let mut budget = limit.saturating_mul(4);
        while let Some([low, high]) = pending.pop() {
            if budget == 0 || self.pieces.len() == limit {
                break;
            }
            budget -= 1;
            self.enclosures += 1;
            let x = Affine::input(low, high);
            match enclose(x, out) {
                Ok(()) => self.push([low, high], x, out),
                // Over the whole interval: the stage itself has no enclosure.
                Err(Undecided::Unsupported) if [low, high] == inputs => break,
                Err(Undecided::Unsupported) => {}
                Err(Undecided::Split { form, at, count }) => {
                    // No more parts pending than pieces may still come, but
                    // at least the two around one threshold.
                    let room = limit - self.pieces.len();
                    let count = count.min(room.saturating_sub(pending.len())).max(1);
                    bytes.reserve_capacity(&mut pending, count + 1)?;
                    split([low, high], x, form, (at, count), &mut pending);
                }
            }
        }
        let released = pending.capacity() * size_of::<[f64; 2]>();
        drop(pending);
        bytes.refund(released);
        shrink(&mut self.pieces, bytes);
        shrink(&mut self.terms, bytes);
        self.index(inputs, bytes)
    }

    /// Builds the bucket index over `inputs`, two buckets or more a piece,
    /// charging it to `bytes`.
    fn index(&mut self, [low, high]: [f64; 2], bytes: &mut Meter) -> pagura_core::Result<()> {
        let buckets = (2 * self.pieces.len()).next_power_of_two();
        bytes.reserve(&mut self.index, buckets)?;
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
        Ok(())
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

    /// The piece that approximates the stage at every input within
    /// `error` of `x` ([`Piece`]), or `None` where no piece holds all those
    /// inputs, its polynomials are too steep for `error` or too far from
    /// the exact values, or a channel bounds its power per sample. `hint`
    /// is a piece to start from: inputs along a line take neighbouring
    /// pieces.
    ///
    /// Every input within `error` of an input the piece holds lies in the
    /// piece, where the exact value at any of its inputs is within the
    /// radius `R` of the polynomial evaluated there, and the polynomial
    /// changes by at most its slope bound `S` times `error` between the two;
    /// so the value written is within `2R + S·error` of the exact one. A
    /// piece serves only where that bound, rounded up, is within
    /// [`APPROXIMATE`] for every channel.
    #[inline]
    pub fn piece(&self, x: f64, error: f64, hint: &mut usize) -> Option<Piece<'_>> {
        let mut i = (*hint).min(self.pieces.len().saturating_sub(1));
        while let Some(&[_, high]) = self.pieces.get(i)
            && high < x
        {
            i += 1;
        }
        while let Some(&[low, _]) = self.pieces.get(i)
            && x < low
            && i > 0
        {
            i -= 1;
        }
        *hint = i;
        let &[low, high] = self.pieces.get(i)?;
        let start = i * self.channels;
        let terms = self.terms.get(start..start + self.channels)?;
        let mut piece = Piece {
            low: low + error,
            high: high - error,
            terms,
            radii: [f64::INFINITY; MAX_RAMP_CHANNELS],
        };
        if !piece.holds(x) {
            return None;
        }
        for (radius, term) in piece.radii.iter_mut().zip(terms) {
            *radius = term.bound(error);
            #[expect(
                clippy::neg_cmp_op_on_partial_ord,
                reason = "a `NaN` bound fails as one above `APPROXIMATE` does"
            )]
            if !(*radius <= APPROXIMATE) {
                return None;
            }
        }
        Some(piece)
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
            slope: 0.0,
        }; MAX_RAMP_CHANNELS];
        for (term, &channel) in terms.iter_mut().zip(out) {
            let Some(t) = Term::new(channel, x, [low, high]) else {
                return;
            };
            *term = Term {
                slope: t.slope_on([low, high]),
                ..t
            };
        }
        self.pieces.push([low, high]);
        self.terms.extend_from_slice(&terms[..out.len()]);
    }
}

impl Term {
    /// `S`, a bound of the polynomial's slope on the piece `low..=high`
    /// (`|k₁| + 2|k₂|h + 3|k₃|h²`, `h` the largest distance from the
    /// centre), rounded up.
    fn slope_on(&self, [low, high]: [f64; 2]) -> f64 {
        let h =
            (high - self.centre).abs().max((self.centre - low).abs()) * (1.0 + 4.0 * f64::EPSILON);
        let [_, k1, k2, k3] = self.k.map(f64::abs);
        (k1 + h * (2.0 * k2 + h * 3.0 * k3)) * (1.0 + 8.0 * f64::EPSILON)
    }

    /// `2R + S·error` on its piece, rounded up: how far the polynomial
    /// evaluated at an input the piece holds lies from the exact value at
    /// any input within `error` of it ([`Ramp::piece`]); zero for an exact
    /// value. Infinite for a power bounded per sample, whose line is the
    /// linear value, not the encoded one; `NaN` for an unbounded slope at no
    /// error.
    fn bound(&self, error: f64) -> f64 {
        if self.radius == 0.0 {
            return 0.0;
        }
        if self.margin != 0.0 {
            return f64::INFINITY;
        }
        // Doubling is exact; the product and the sum round once each.
        (2.0 * self.radius + self.slope * error) * (1.0 + 4.0 * f64::EPSILON)
    }

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
                slope: 0.0,
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
            slope: 0.0,
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
                slope: 0.0,
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

/// `k₀ + τ·(k₁ + τ·(k₂ + τ·k₃))`, as every term evaluates.
#[inline]
fn horner([k0, k1, k2, k3]: [f64; 4], t: f64) -> f64 {
    k0 + t * (k1 + t * (k2 + t * k3))
}

/// Shrinks `values` to its length, charging the replacement to `bytes`
/// before the old storage is released and refunding the old storage after;
/// leaves it as it is where `bytes` lacks room for both.
fn shrink<T>(values: &mut Vec<T>, bytes: &mut Meter) {
    let old = values.capacity();
    if values.len() == old || bytes.charge_items::<T>(values.len()).is_err() {
        return;
    }
    values.shrink_to_fit();
    // What is held now stays charged.
    bytes.refund((old + values.len() - values.capacity()) * size_of::<T>());
}

/// Pushes onto `pending`, the leftmost last, the parts of `low..=high`
/// (whose input form is `x`) between where `form` reaches `at`, `at + 1`,
/// …, `at + count − 1`, leaving out the inputs within twice `form`'s radius
/// of each threshold, whose choice the enclosure cannot tell. A part that
/// would not shrink the interval is dropped.
fn split(
    [low, high]: [f64; 2],
    x: Affine,
    form: Affine,
    (at, count): (f64, usize),
    pending: &mut Vec<[f64; 2]>,
) {
    if form.s == 0.0 || x.s == 0.0 {
        return;
    }
    let width = 2.0 * (form.r + f64::EPSILON * (form.c.abs() + form.s.abs() + form.r))
        / form.s.abs()
        + 1.0 / (1_u64 << 40) as f64;
    // The inputs at `ξ` rise with it, and so do the thresholds' where
    // `form` rises.
    let input = |xi: f64| x.c + x.s * xi;
    let first = pending.len();
    let mut start = low;
    for i in 0..count {
        let k = if form.s > 0.0 { i } else { count - 1 - i };
        let centre = (at + usize_f64(k) - form.c) / form.s;
        let (before, after) = (input(centre - width), input(centre + width));
        let end = before.next_down().min(high.next_down());
        if end >= start && end < high {
            pending.push([start, end]);
        }
        start = start
            .max(end.next_up())
            .max(after.next_up())
            .max(low.next_up());
    }
    if start <= high && start > low {
        pending.push([start, high]);
    }
    pending[first..].reverse();
}

#[cfg(test)]
mod tests;
