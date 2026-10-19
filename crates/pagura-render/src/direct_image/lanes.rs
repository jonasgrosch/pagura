//! Separable reconstruction in fixed, independent f32 lanes.
#![expect(
    clippy::inline_always,
    reason = "fixed output footprints expose independent lanes"
)]
use super::footprint::{Band, Tap};

#[inline]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::manual_clamp,
    reason = "rounded samples are clamped to the byte range"
)]
fn quantize(value: f32) -> u8 {
    (value.max(0.0).min(255.0) as u32).to_le_bytes()[0]
}

/// Reduces one source row, retaining normalized fractional samples.
pub(super) fn horizontal(
    source: &[[f32; 4]],
    spans: &[Band],
    taps: &[Tap],
    output: &mut [[f32; 4]],
) {
    for (pixel, span) in output.iter_mut().zip(spans) {
        let mut sum = [0.0f32; 4];
        for tap in &taps[span.start..span.end] {
            let input = source[tap.sample];
            for lane in 0..4 {
                sum[lane] += input[lane] * tap.weight;
            }
        }
        *pixel = sum;
    }
}
/// Accumulates one row with a vertical footprint weight.
pub(super) fn accumulate<const N: usize>(
    source: &[[f32; N]],
    weight: f32,
    output: &mut [[f32; N]],
) {
    let (input, tail) = source.as_flattened().as_chunks::<8>();
    let (target, remainder) = output.as_flattened_mut().as_chunks_mut::<8>();
    for (out, pixel) in target.iter_mut().zip(input) {
        for lane in 0..8 {
            out[lane] += pixel[lane] * weight;
        }
    }
    for (out, &pixel) in remainder.iter_mut().zip(tail) {
        *out += pixel * weight;
    }
}
/// Rounds independent channels to bytes.
pub(super) fn pack(source: &[[f32; 4]], output: &mut [[u8; 4]]) {
    for (out, pixel) in output.iter_mut().zip(source) {
        *out = pixel.map(|v| quantize(v + 0.5));
    }
}
/// Packs one-channel samples to opaque RGBA without intermediate expansion.
pub(super) fn pack_gray(source: &[f32], output: &mut [[u8; 4]]) {
    for (out, &pixel) in output.iter_mut().zip(source) {
        *out = gray_pixel(pixel);
    }
}
/// Combines up to three cached rows, preserving tap order, and packs bytes.
#[must_use]
pub(super) fn combine_pack(
    rows: [&[[f32; 4]]; 3],
    weights: [f32; 3],
    count: usize,
    output: &mut [[u8; 4]],
) -> bool {
    if count > 3 || rows[..count].iter().any(|r| r.len() < output.len()) {
        return false;
    }
    let wide = output.len() / 4 * 4;
    for (group, out) in output[..wide].as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let mut sum = [0.0f32; 16];
        for tap in 0..count {
            let input = rows[tap][group * 4..group * 4 + 4].as_flattened();
            for lane in 0..16 {
                sum[lane] += input[lane] * weights[tap];
            }
        }
        let bytes = sum.map(|v| quantize(v + 0.5));
        out.as_flattened_mut().copy_from_slice(&bytes);
    }
    for (i, out) in output[wide..].iter_mut().enumerate() {
        let mut sum = [0.0f32; 4];
        for tap in 0..count {
            for lane in 0..4 {
                sum[lane] += rows[tap][wide + i][lane] * weights[tap];
            }
        }
        *out = sum.map(|v| quantize(v + 0.5));
    }
    true
}
/// Combines up to three one-channel rows directly to opaque RGBA bytes.
#[must_use]
pub(super) fn combine_gray(
    rows: [&[f32]; 3],
    weights: [f32; 3],
    count: usize,
    output: &mut [[u8; 4]],
) -> bool {
    if count > 3 || rows[..count].iter().any(|r| r.len() < output.len()) {
        return false;
    }
    let wide = output.len() / 4 * 4;
    for (i, out) in output[..wide].as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let mut sum = [0.0f32; 4];
        for tap in 0..count {
            let input = &rows[tap][i * 4..i * 4 + 4];
            for lane in 0..4 {
                sum[lane] += input[lane] * weights[tap];
            }
        }
        for (out, v) in out.iter_mut().zip(sum) {
            *out = gray_pixel(v);
        }
    }
    for (i, out) in output[wide..].iter_mut().enumerate() {
        let mut sum = 0.0;
        for tap in 0..count {
            sum += rows[tap][wide + i] * weights[tap];
        }
        *out = gray_pixel(sum);
    }
    true
}
#[inline]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::manual_clamp,
    reason = "gray channels are rounded and clamped to the byte domain"
)]
fn gray_pixel(value: f32) -> [u8; 4] {
    let gray = (value + 0.5).max(0.0).min(255.0) as u32;
    (gray | (gray << 8) | (gray << 16) | 0xff00_0000).to_le_bytes()
}

/// A bounded structure-of-arrays plan for one-channel reconstruction.
#[derive(Debug)]
pub(super) struct GrayPlan {
    indices: Vec<[usize; 4]>,
    weights: Vec<[f32; 4]>,
    groups: Vec<Band>,
    width: usize,
    maximum: usize,
}
impl GrayPlan {
    /// Prepares groups of four footprints without exceeding `budget` bytes.
    #[must_use]
    pub(super) fn new(spans: &[Band], taps: &[Tap], budget: usize) -> Option<Self> {
        let mut count = 0usize;
        let mut maximum = 0usize;
        for group in spans.chunks(4) {
            let mut longest = 0;
            for span in group {
                let input = taps.get(span.start..span.end)?;
                longest = longest.max(input.len());
                for tap in input {
                    maximum = maximum.max(tap.sample.checked_add(1)?);
                }
            }
            count = count.checked_add(longest)?;
        }
        let groups_count = spans.len().div_ceil(4);
        let bytes = count
            .checked_mul(size_of::<[usize; 4]>() + size_of::<[f32; 4]>())?
            .checked_add(groups_count.checked_mul(size_of::<Band>())?)?;
        if bytes > budget {
            return None;
        }
        let mut plan = Self {
            indices: Vec::new(),
            weights: Vec::new(),
            groups: Vec::new(),
            width: spans.len(),
            maximum,
        };
        plan.indices.try_reserve_exact(count).ok()?;
        plan.weights.try_reserve_exact(count).ok()?;
        plan.groups.try_reserve_exact(groups_count).ok()?;
        for group in spans.chunks(4) {
            let start = plan.weights.len();
            let longest = group.iter().map(|p| p.end - p.start).max().unwrap_or(0);
            for rank in 0..longest {
                let mut index = [0; 4];
                let mut weights = [0.0; 4];
                for (lane, span) in group.iter().enumerate() {
                    if rank < span.end - span.start {
                        let tap = &taps[span.start + rank];
                        index[lane] = tap.sample;
                        weights[lane] = tap.weight;
                    }
                }
                plan.indices.push(index);
                plan.weights.push(weights);
            }
            plan.groups.push(Band {
                start,
                end: plan.weights.len(),
            });
        }
        Some(plan)
    }
    /// Evaluates a source row, returning false for incompatible buffer extents.
    #[must_use]
    pub(super) fn apply(&self, source: &[f32], output: &mut [f32]) -> bool {
        if source.len() < self.maximum || output.len() < self.width {
            return false;
        }
        self.evaluate(source, 1.0, output);
        true
    }
    /// Evaluates integer source codes without a full floating-point row buffer.
    #[must_use]
    pub(super) fn apply_bytes(&self, source: &[u8], factor: f32, output: &mut [f32]) -> bool {
        if source.len() < self.maximum || output.len() < self.width {
            return false;
        }
        self.evaluate(source, factor, output);
        true
    }
    #[inline(always)]
    fn evaluate<T: Copy + Into<f32>>(&self, source: &[T], factor: f32, output: &mut [f32]) {
        for (group, span) in self.groups.iter().enumerate() {
            let mut sum = [0.0f32; 4];
            for i in span.start..span.end {
                let index = self.indices[i];
                let input = index.map(|x| source[x].into());
                let weight = self.weights[i];
                for lane in 0..4 {
                    sum[lane] += input[lane] * weight[lane];
                }
            }
            let sum = sum.map(|v| v * factor);
            let first = group * 4;
            let count = (self.width - first).min(4);
            if count == 4 {
                output[first..first + 4].copy_from_slice(&sum);
            } else {
                let values = sum;
                output[first..first + count].copy_from_slice(&values[..count]);
            }
        }
    }
}
