//! The page buffer into RGBA8 (11.3.3): process colour into RGB, spot
//! colorant equivalents added for overprint preview, the selected transfer
//! function (11.7.5.2), then source-over onto the background premultiplied
//! once, each channel quantized once with `quantize_unit`.

use super::frame::CHANNELS;
use super::{Cover, PageBuffer, Usage, Workspace};
use crate::plan::{Output, Plan};
use crate::process;
use pagura_color::{Conversion, Encoder, Process};
use pagura_core::convert::u32_to_usize;
use pagura_raster::quantize_unit;
use std::ops::Range;

/// Records `select` for the fully covered pixels of a span at `(x, y)` of
/// the band starting at row `first`: the last opaque elementary mark
/// covering a pixel selects its transfer function.
pub(super) fn record(
    plan: &Plan,
    selection: &mut [u16],
    (x, y): (u32, u32),
    first: u32,
    (cover, select): (Cover<'_>, u16),
) {
    let region = plan.area.region;
    let width = u32_to_usize(region.width());
    let begin = u32_to_usize(y - first) * width + u32_to_usize(x - region.x0);
    let Some(row) = selection.get_mut(begin..begin + cover.len()) else {
        return;
    };
    for (k, slot) in row.iter_mut().enumerate() {
        if cover.at(k) == 255 {
            *slot = select;
        }
    }
}

/// Appends the band's rows of the page buffer to `out` as premultiplied
/// RGBA8, whole rows of the region.
pub(super) fn write(plan: &Plan, ws: &mut Workspace, rows: &Range<u32>, out: &mut Vec<u8>) {
    let width = u32_to_usize(plan.area.region.width());
    let output = &plan.output;
    let mut preview = Preview::default();
    // One fill of the whole band, written over while it is still in cache:
    // cheaper than filling a surface beforehand, and than appending row by
    // row. A background of one repeated byte, as white and transparent
    // are, is that fill; any other is written where no store reached.
    let [byte, ..] = output.background;
    let uniform = output.background.iter().all(|&b| b == byte);
    let start = out.len();
    out.resize(
        start + rows.len() * width * 4,
        if uniform { byte } else { 0 },
    );
    let band = out.get_mut(start..).unwrap_or_default();
    for (band_row, row) in band.chunks_exact_mut(width * 4).enumerate() {
        let row = row.as_chunks_mut::<4>().0;
        ws.page
            .drain_row(band_row, width, output, |pixels, (lo, hi)| {
                let (head, rest) = row.split_at_mut(lo);
                let (row, tail) = rest.split_at_mut(hi - lo);
                if !uniform {
                    head.fill(output.background);
                    tail.fill(output.background);
                }
                let first = band_row * width + lo;
                let end = first + row.len();
                match pixels {
                    PageBuffer::N16(words) => {
                        if let Some(words) = words.get(first..end) {
                            crate::kernel::n16_bytes(row, words);
                        }
                    }
                    PageBuffer::F32(levels) => {
                        if let Some(levels) = levels.get(first..end) {
                            crate::kernel::f32_bytes(row, levels);
                        }
                    }
                    PageBuffer::F32Cmyk(levels) => {
                        if let Some(levels) = levels.get(first..end) {
                            cmyk_levels(output, row, levels);
                        }
                    }
                    PageBuffer::F64(values) => {
                        let stride = output.layout.stride();
                        let pixels = values.get(first * stride..).unwrap_or_default();
                        let pixels = pixels.chunks_exact(stride);
                        let selection = ws.selection.get(first..).unwrap_or_default();
                        let plain = output.spots.is_empty() && output.transfers.is_empty();
                        match output.layout.channels() {
                            1 if plain => self::plain::<1>(output, row, pixels),
                            3 if plain => self::plain::<3>(output, row, pixels),
                            4 if plain => self::plain::<4>(output, row, pixels),
                            _ => {
                                for (k, (out, pixel)) in row.iter_mut().zip(pixels).enumerate() {
                                    let select = selection.get(k).copied().unwrap_or(0);
                                    *out = wide(output, pixel, select, &mut preview, &mut ws.usage);
                                }
                            }
                        }
                    }
                }
            });
        ws.usage.work = ws.usage.work.saturating_add(width);
    }
}

/// [`wide`] of a row's pixels of `N` process channels, without spot
/// channels or transfer functions: their bytes follow from colour and
/// alpha alone, so a pixel equal to the one before it, as neighbours in
/// flat areas and magnified images are, takes that one's bytes.
fn plain<'a, const N: usize>(
    output: &Output,
    row: &mut [[u8; 4]],
    pixels: impl Iterator<Item = &'a [f64]>,
) {
    let process = output.layout.process;
    let mut last: Option<(&[f64; N], f64, [u8; 4])> = None;
    for (out, pixel) in row.iter_mut().zip(pixels) {
        let Some((color, &[alpha, ..])) = pixel.split_first_chunk::<N>() else {
            continue;
        };
        if let Some((previous, previous_alpha, bytes)) = last
            && previous_alpha.to_bits() == alpha.to_bits()
            && previous
                .iter()
                .zip(color)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        {
            *out = bytes;
            continue;
        }
        *out = if alpha == 0.0 {
            output.background
        } else {
            let straight = color.map(|c| (c / alpha).clamp(0.0, 1.0));
            over(output.background, process::rgb(process, &straight), alpha)
        };
        last = Some((color, alpha, *out));
    }
}

/// [`plain`] of a row of a CMYK page's F32 levels, in f64: a pixel's
/// straight values are its colour levels over its alpha level, by at most
/// one division (none for an opaque pixel).
fn cmyk_levels(output: &Output, row: &mut [[u8; 4]], levels: &[[f32; 5]]) {
    let mut last: Option<([f32; 5], [u8; 4])> = None;
    for (out, &pixel) in row.iter_mut().zip(levels) {
        if let Some((previous, bytes)) = last
            && previous
                .iter()
                .zip(&pixel)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        {
            *out = bytes;
            continue;
        }
        let [c, m, y, k, alpha] = pixel.map(f64::from);
        *out = if alpha == 0.0 {
            output.background
        } else {
            let unit = if alpha == 255.0 {
                1.0 / 255.0
            } else {
                1.0 / alpha
            };
            let straight = [c, m, y, k].map(|v| (v * unit).clamp(0.0, 1.0));
            let rgb = process::rgb(Process::Cmyk, &straight);
            over(output.background, rgb, alpha * (1.0 / 255.0))
        };
        last = Some((pixel, *out));
    }
}

/// One f64 page pixel as premultiplied RGBA8 over the background.
fn wide(
    output: &Output,
    pixel: &[f64],
    select: u16,
    preview: &mut Preview,
    usage: &mut Usage,
) -> [u8; 4] {
    let layout = output.layout;
    let n = layout.channels();
    let alpha = pixel[n];
    if alpha == 0.0 {
        return output.background;
    }
    let mut straight = [0.0; CHANNELS];
    for (value, &c) in straight.iter_mut().zip(&pixel[..n]) {
        *value = (c / alpha).clamp(0.0, 1.0);
    }
    let process = layout.process.components();
    let mut rgb = if output.spots.is_empty() {
        process::rgb(layout.process, &straight[..process])
    } else {
        preview.rgb(output, &straight[..n], usage)
    };
    if let Some(functions) = select
        .checked_sub(1)
        .and_then(|i| output.transfers.get(usize::from(i)))
    {
        for (value, function) in rgb.iter_mut().zip(functions) {
            if let Some(function) = function {
                *value = function.map(value.clamp(0.0, 1.0)).clamp(0.0, 1.0);
                usage.functions = usage.functions.saturating_add(function.cost());
            }
        }
    }
    over(output.background, rgb, alpha)
}

/// Source-over onto the premultiplied background, quantized once.
fn over(background: [u8; 4], rgb: [f64; 3], alpha: f64) -> [u8; 4] {
    if alpha == 1.0 {
        let [r, g, b] = rgb.map(quantize_unit);
        return [r, g, b, 255];
    }
    let mut out = background;
    for (out, value) in out.iter_mut().zip(rgb) {
        *out = quantize_unit(value * alpha + f64::from(*out) / 255.0 * (1.0 - alpha));
    }
    out[3] = quantize_unit(alpha + f64::from(background[3]) / 255.0 * (1.0 - alpha));
    out
}

/// Overprint preview (ADR 0004 §7): process colour as CMYK plus the CMYK
/// equivalents of the spot tints, added with saturation, then RGB. The last
/// tints and their equivalent are kept, compared exactly.
#[derive(Debug, Default)]
struct Preview {
    last: Option<([f64; crate::plan::MAX_SPOTS], [f64; 4])>,
}

impl Preview {
    fn rgb(&mut self, output: &Output, straight: &[f64], usage: &mut Usage) -> [f64; 3] {
        let layout = output.layout;
        let process = layout.process.components();
        let mut cmyk = [0.0; 4];
        if layout.process == Process::Cmyk {
            cmyk.copy_from_slice(&straight[..4]);
        } else {
            let rgb = process::rgb(layout.process, &straight[..process]);
            Encoder::new(Process::Cmyk, &Conversion::default()).encode(rgb, &mut cmyk);
        }
        let mut tints = [0.0; crate::plan::MAX_SPOTS];
        for (tint, &value) in tints.iter_mut().zip(&straight[process..]) {
            *tint = 1.0 - value;
        }
        let equivalent = match self.last {
            Some((previous, equivalent)) if previous == tints => equivalent,
            _ => {
                let equivalent = equivalent(output, &tints, usage);
                self.last = Some((tints, equivalent));
                equivalent
            }
        };
        let mixed = [0, 1, 2, 3].map(|i| (cmyk[i] + equivalent[i] - 1.0).max(0.0));
        process::rgb(Process::Cmyk, &mixed)
    }
}

/// The CMYK equivalent of spot tints, each definition's alternate added with
/// saturation.
fn equivalent(
    output: &Output,
    tints: &[f64; crate::plan::MAX_SPOTS],
    usage: &mut Usage,
) -> [f64; 4] {
    let mut result = [1.0; 4];
    for spot in &output.spots {
        let mut input = [0.0; pagura_color::MAX_COMPONENTS];
        for &(component, channel) in &spot.inputs {
            if let (Some(slot), Some(&tint)) = (input.get_mut(component), tints.get(channel)) {
                *slot = tint;
            }
        }
        let components = spot.components.min(input.len());
        if input[..components].iter().all(|&v| v == 0.0) {
            continue;
        }
        let mut value = [0.0; 4];
        spot.convert.convert(&input[..components], &mut value);
        usage.functions = usage.functions.saturating_add(spot.convert.cost());
        for (out, v) in result.iter_mut().zip(value) {
            *out = (*out + v - 1.0).max(0.0);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_matches_the_earlier_output_arithmetic() {
        assert_eq!(
            over([17, 85, 170, 64], [0.5, 0.25, 1.0], 1.0),
            [128, 64, 255, 255]
        );
        let pixel = over([0, 64, 128, 64], [0.5, 1.0, 1.0], 0.5);
        assert_eq!(
            pixel,
            [
                quantize_unit(0.25),
                quantize_unit(0.5 + 64.0 / 255.0 * 0.5),
                quantize_unit(0.5 + 128.0 / 255.0 * 0.5),
                quantize_unit(0.5 + 64.0 / 255.0 * 0.5),
            ]
        );
    }

    #[test]
    fn cmyk_levels_round_their_exact_conversion() {
        // Levels on and between whole values, transparent and translucent
        // pixels, values past alpha and repeated pixels: every byte is a
        // nearest rounding of a value within 1e-9 of the exact conversion.
        let output = Output {
            layout: crate::plan::Layout {
                process: Process::Cmyk,
                spots: 0,
            },
            background: [255, 128, 0, 192],
            spots: Vec::new(),
            transfers: Vec::new(),
        };
        let samples = [0.0, 1.0, 63.75, 127.5, 200.25, 255.0];
        let levels: Vec<[f32; 5]> = (0..96)
            .map(|k| {
                let pick = |i: usize| samples[(k / 2 + i) % samples.len()];
                let alpha = [255.0, 127.5, 0.0, 63.75][k / 8 % 4];
                [pick(0), pick(1), pick(2), pick(3), alpha]
            })
            .collect();
        let mut row = vec![[0; 4]; levels.len()];
        cmyk_levels(&output, &mut row, &levels);
        let background = output.background.map(|b| f64::from(b) / 255.0);
        for (bytes, pixel) in row.iter().zip(&levels) {
            let [c, m, y, k, alpha] = pixel.map(|l| f64::from(l) / 255.0);
            if alpha == 0.0 {
                assert_eq!(*bytes, output.background);
                continue;
            }
            let straight = [c, m, y, k].map(|v| (v / alpha).clamp(0.0, 1.0));
            let rgb = process::rgb(Process::Cmyk, &straight);
            let exact = [0, 1, 2]
                .map(|i| rgb[i] * alpha + background[i] * (1.0 - alpha))
                .into_iter()
                .chain([alpha + background[3] * (1.0 - alpha)]);
            for (&byte, value) in bytes.iter().zip(exact) {
                assert!(
                    (f64::from(byte) - 255.0 * value).abs() <= 0.5 + 1e-9,
                    "{pixel:?}"
                );
            }
        }
    }

    #[test]
    fn plain_rows_give_the_bytes_of_each_pixel() {
        // Pairs of equal pixels, signed zeros, values past alpha, NaN.
        let samples = [0.0, -0.0, 0.2, 0.5, 1.0, 1.25, f64::NAN];
        for process in [Process::Gray, Process::Rgb, Process::Cmyk] {
            let output = Output {
                layout: crate::plan::Layout { process, spots: 0 },
                background: [255, 128, 0, 192],
                spots: Vec::new(),
                transfers: Vec::new(),
            };
            let n = output.layout.channels();
            let mut values = Vec::new();
            for k in 0..64 {
                let pick = |i: usize| samples[(k / 2 + i) % samples.len()];
                values.extend((0..n).map(pick));
                values.extend([pick(k % 3), 0.0, 0.0]);
            }
            let pixels = values.chunks_exact(output.layout.stride());
            let mut row = vec![[0; 4]; 64];
            match n {
                1 => plain::<1>(&output, &mut row, pixels.clone()),
                3 => plain::<3>(&output, &mut row, pixels.clone()),
                _ => plain::<4>(&output, &mut row, pixels.clone()),
            }
            let (mut preview, mut usage) = (Preview::default(), Usage::default());
            for (bytes, pixel) in row.iter().zip(pixels) {
                assert_eq!(*bytes, wide(&output, pixel, 0, &mut preview, &mut usage));
            }
        }
    }
}
