//! The page buffer into RGBA8 (11.3.3): process colour into RGB, spot
//! colorant equivalents added for overprint preview, the selected transfer
//! function (11.7.5.2), then source-over onto the background premultiplied
//! once, each channel quantized once with `quantize_unit`.

use super::frame::CHANNELS;
use super::{Cover, PageBuffer, Usage, Workspace};
use crate::plan::{Output, Plan};
use crate::process::{self, Separation};
use pagura_color::Process;
use pagura_core::convert::u32_to_usize;
use pagura_raster::{Pixmap, quantize_unit};
use std::ops::Range;

/// Records `select` for the fully covered pixels of a span at `(x, y)` of
/// the band starting at row `first`: the last opaque elementary mark
/// covering a pixel selects its transfer function.
pub(super) fn record(
    plan: &Plan,
    selection: &mut [u16],
    (x, y, first): (u32, u32, u32),
    cover: Cover<'_>,
    select: u16,
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

/// Writes the band's rows of the page buffer to `target`, whose pixel
/// `(0, 0)` is the region's first pixel.
pub(super) fn write(plan: &Plan, ws: &mut Workspace, rows: &Range<u32>, target: &mut Pixmap) {
    let region = plan.area.region;
    let width = u32_to_usize(region.width());
    let target_width = u32_to_usize(target.width());
    let target_height = target.height();
    let count = width.min(target_width);
    let output = &plan.output;
    let mut preview = Preview::default();
    let data = target.data_mut();
    for (band_row, y) in rows.clone().enumerate() {
        let Some(row) = y.checked_sub(region.y0).filter(|&r| r < target_height) else {
            continue;
        };
        let start = u32_to_usize(row) * target_width * 4;
        let Some(out) = data.get_mut(start..start + count * 4) else {
            continue;
        };
        let out = out.as_chunks_mut::<4>().0;
        // Columns no store reached still hold the background.
        let (lo, hi) = ws
            .extents
            .get(band_row)
            .map_or((0, 0), |(lo, hi)| (lo.min(count), hi.min(count)));
        let (head, rest) = out.split_at_mut(lo);
        let (out, tail) = rest.split_at_mut(hi - lo);
        head.fill(output.background);
        tail.fill(output.background);
        let first = band_row * width + lo;
        match &ws.page {
            PageBuffer::N16(words) => {
                for (out, word) in out.iter_mut().zip(words.iter().skip(first)) {
                    *out = word.map(nearest_byte);
                }
            }
            PageBuffer::F32(levels) => {
                for (out, level) in out.iter_mut().zip(levels.iter().skip(first)) {
                    *out = level.map(byte);
                }
            }
            PageBuffer::U8(bytes) => {
                for (out, pixel) in out.iter_mut().zip(bytes.iter().skip(first)) {
                    *out = *pixel;
                }
            }
            PageBuffer::F64(values) => {
                let stride = output.layout.stride();
                let pixels = values.get(first * stride..).unwrap_or_default();
                let selection = ws.selection.get(first..).unwrap_or_default();
                for (k, (out, pixel)) in out.iter_mut().zip(pixels.chunks_exact(stride)).enumerate()
                {
                    let select = selection.get(k).copied().unwrap_or(0);
                    *out = wide(output, pixel, select, &mut preview, &mut ws.usage);
                }
            }
        }
        ws.usage.work = ws.usage.work.saturating_add(count);
    }
}

/// The nearest byte of an N16 word: `(w + 128) / 257`, never a tie.
#[expect(
    clippy::cast_possible_truncation,
    reason = "(65535 + 128) / 257 is 255"
)]
fn nearest_byte(word: u16) -> u8 {
    ((u32::from(word) + 128) / 257) as u8
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a level rounded and clamped to a byte"
)]
fn byte(level: f32) -> u8 {
    level.round().clamp(0.0, 255.0) as u8
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
            process::encode_rgb(Process::Cmyk, rgb, &Separation::default(), &mut cmyk);
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
}
