//! RGB output without colour-space or transfer dispatch per pixel (11.3.3).
//! Bounded page storage rounds each level once to the nearest byte (ADR 0004).

use super::{Context, Model, Pixel, Surface, colorants};
use crate::Tile;
use crate::device::quantize_unit;
use pagura_core::{Error, Result};
use pagura_raster::Pixmap;

impl Context {
    pub(super) fn write_output(
        &mut self,
        page: &Surface,
        spots: &[Surface],
        model: Model,
        output: &mut Pixmap,
        tile: Tile,
    ) -> Result<()> {
        let Tile {
            x,
            y,
            width,
            height,
        } = page.tile;
        let stride =
            usize::try_from(tile.width).map_err(|_| Error::limit("output width exceeds usize"))?;
        let origin = usize::try_from(y - tile.y)
            .map_err(|_| Error::limit("output row exceeds usize"))?
            * stride
            + usize::try_from(x - tile.x)
                .map_err(|_| Error::limit("output column exceeds usize"))?;
        let columns =
            usize::try_from(width).map_err(|_| Error::limit("chunk width exceeds usize"))?;
        if let Some(buffer) = page.narrow() {
            // Admission keeps spot passes and transfer feeds on f64 storage.
            for row in 0..pagura_core::convert::u32_to_usize(height) {
                let start = origin + row * stride;
                let out = output.data_mut().get_mut(start * 4..(start + columns) * 4);
                debug_assert!(out.is_some(), "chunks lie inside the output tile");
                if let Some(out) = out {
                    buffer.output(row * columns, out);
                }
            }
            return Ok(());
        }
        for (row, pixels) in page.pixels().chunks_exact(columns).enumerate() {
            let start = origin + row * stride;
            let out = output
                .data_mut()
                .get_mut(start * 4..(start + pixels.len()) * 4);
            debug_assert!(out.is_some(), "chunks lie inside the output tile");
            let Some(out) = out else { continue };
            let out = out.as_chunks_mut::<4>().0;
            if model == Model::Rgb && spots.is_empty() && self.transfers.is_none() {
                rgb(out, pixels);
                continue;
            }
            for (column, (target, pixel)) in out.iter_mut().zip(pixels).enumerate() {
                let alpha = pixel.alpha;
                if alpha == 0.0 {
                    continue;
                }
                let color = if spots.is_empty() {
                    model.rgb(pixel.straight())
                } else {
                    // Spot planes share the page surface's tile, four inks each.
                    let index = row * columns + column;
                    let inks = spots.iter().flat_map(|surface| {
                        surface
                            .pixels()
                            .get(index)
                            .map_or([0.0; 4], |p| p.straight())
                    });
                    let mut tints = [0.0; colorants::MAX_SPOTS];
                    for (tint, ink) in tints.iter_mut().zip(inks) {
                        *tint = 1.0 - ink;
                    }
                    self.charge(self.separations.len())?;
                    self.preview.rgb(
                        &self.separations,
                        model,
                        pixel.straight(),
                        &tints,
                        &mut self.function_work,
                    )?
                };
                let color = self.transfer_color(row * pixels.len() + column, color)?;
                composite_output(target, color, alpha);
            }
        }
        Ok(())
    }
}

fn rgb(output: &mut [[u8; 4]], pixels: &[Pixel]) {
    for (target, pixel) in output.iter_mut().zip(pixels) {
        if pixel.alpha == 0.0 {
            continue;
        }
        if pixel.alpha == 1.0 {
            // Division by unit alpha is exact; quantize_unit applies the same clamp.
            *target = [
                quantize_unit(pixel.color[0]),
                quantize_unit(pixel.color[1]),
                quantize_unit(pixel.color[2]),
                255,
            ];
        } else {
            let [red, green, blue, _] = pixel.straight();
            composite_output(target, [red, green, blue], pixel.alpha);
        }
    }
}

/// Source-over onto the output background, quantized once (11.3.3).
fn composite_output(target: &mut [u8; 4], color: [f64; 3], alpha: f64) {
    if alpha == 1.0 {
        let [red, green, blue] = color.map(quantize_unit);
        *target = [red, green, blue, 255];
        return;
    }
    for (i, out) in target.iter_mut().enumerate().take(3) {
        *out = quantize_unit(color[i] * alpha + f64::from(*out) / 255.0 * (1.0 - alpha));
    }
    target[3] = quantize_unit(alpha + f64::from(target[3]) / 255.0 * (1.0 - alpha));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_preserves_opaque_clamping_and_rational_source_over() {
        let mut output = [[17, 85, 170, 64], [0, 64, 128, 64], [17, 85, 170, 64]];
        let pixels = [
            Pixel::source([0.5, 0.25, 2.0, 0.0], 1.0),
            Pixel::source([0.25, 0.5, 1.0, 0.0], 0.5),
            Pixel::default(),
        ];
        rgb(&mut output, &pixels);
        // Exact halves and quarters, with one final half-up byte rounding.
        assert_eq!(
            output,
            [[128, 64, 255, 255], [32, 96, 192, 160], [17, 85, 170, 64]]
        );
    }

    #[test]
    fn direct_rgb_output_matches_the_general_f64_evaluation() {
        let values = [
            -0.0,
            0.0,
            f64::MIN_POSITIVE,
            0.3,
            0.5,
            1.0,
            f64::INFINITY,
            f64::NAN,
        ];
        for alpha in [0.0, 0.5, 1.0] {
            for color in values {
                for background in [[0; 4], [17, 35, 61, 96], [255; 4]] {
                    let pixel = Pixel {
                        color: [color; 4],
                        alpha,
                        ..Pixel::default()
                    };
                    let mut expected = background;
                    if alpha != 0.0 {
                        let color = Model::Rgb.rgb(pixel.straight());
                        for (i, out) in expected.iter_mut().enumerate().take(3) {
                            *out = quantize_unit(
                                color[i] * alpha + f64::from(*out) / 255.0 * (1.0 - alpha),
                            );
                        }
                        expected[3] =
                            quantize_unit(alpha + f64::from(expected[3]) / 255.0 * (1.0 - alpha));
                    }
                    let mut actual = [background];
                    rgb(&mut actual, &[pixel]);
                    assert_eq!(actual, [expected], "{color} at alpha {alpha}");
                }
            }
        }
    }
}
