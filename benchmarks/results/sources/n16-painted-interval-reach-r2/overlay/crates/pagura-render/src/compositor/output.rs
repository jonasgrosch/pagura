//! RGB output without colour-space or transfer dispatch per pixel (11.3.3).
//! Bounded page storage rounds each level once to the nearest byte (ADR 0004).

use super::{Context, Model, Pixel, Surface, quantize};
use crate::Tile;
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
        if let Some(buffer) = page.narrow() {
            // Admission keeps spot passes and transfer feeds on f64 storage.
            let width = width as usize;
            if let Some(painted) = buffer.painted_range() {
                let mut converted = 0;
                let mut calls = 0;
                // N16 admission requires an opaque page background. The output
                // already holds that background outside this chunk's stores.
                for row in 0..height as usize {
                    let base = row * width;
                    let lo = base.max(painted.start);
                    let hi = (base + width).min(painted.end);
                    if lo >= hi {
                        continue;
                    }
                    let start = origin + row * stride + (lo - base);
                    buffer.output(lo, &mut output.data_mut()[start * 4..(start + hi - lo) * 4]);
                    converted += hi - lo;
                    calls += 1;
                }
                eprintln!(
                    "PAGURA_N16_REACH_V2 output storage=N16 start={} end={} pixels={converted} calls={calls} total={}",
                    painted.start, painted.end, width * height as usize,
                );
                return Ok(());
            }
            for row in 0..height as usize {
                let start = origin + row * stride;
                buffer.output(
                    row * width,
                    &mut output.data_mut()[start * 4..(start + width) * 4],
                );
            }
            eprintln!(
                "PAGURA_N16_REACH_V2 output storage={} dense_pixels={}",
                if matches!(buffer, super::narrow::Buffer::F32(_)) { "F32" } else { "U8" },
                width * height as usize,
            );
            return Ok(());
        }
        for (row, pixels) in page
            .pixels()
            .chunks_exact(
                usize::try_from(width).map_err(|_| Error::limit("chunk width exceeds usize"))?,
            )
            .enumerate()
        {
            let start = origin + row * stride;
            if model == Model::Rgb && spots.is_empty() && self.transfers.is_none() {
                rgb(
                    output.data_mut()[start * 4..(start + pixels.len()) * 4]
                        .as_chunks_mut::<4>()
                        .0,
                    pixels,
                );
                continue;
            }
            for (column, (target, pixel)) in output.data_mut()
                [start * 4..(start + pixels.len()) * 4]
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(pixels)
                .enumerate()
            {
                let alpha = pixel.alpha;
                if alpha == 0.0 {
                    continue;
                }
                let color = if spots.is_empty() {
                    model.rgb(pixel.straight())
                } else {
                    let index = row
                        * usize::try_from(width)
                            .map_err(|_| Error::limit("chunk width exceeds usize"))?
                        + column;
                    let mut tints = [0.0; 32];
                    for (batch, surface) in spots.iter().enumerate() {
                        for (i, value) in surface.pixels()[index].straight().into_iter().enumerate()
                        {
                            tints[batch * 4 + i] = 1.0 - value;
                        }
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
        eprintln!(
            "PAGURA_N16_REACH_V2 output storage=F64 visited_pixels={}",
            u64::from(width) * u64::from(height),
        );
        Ok(())
    }
}

fn rgb(output: &mut [[u8; 4]], pixels: &[Pixel]) {
    for (target, pixel) in output.iter_mut().zip(pixels) {
        if pixel.alpha == 0.0 {
            continue;
        }
        if pixel.alpha == 1.0 {
            // Division by unit alpha is exact; quantize applies the same clamp.
            *target = [
                quantize(pixel.color[0]),
                quantize(pixel.color[1]),
                quantize(pixel.color[2]),
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
        let [red, green, blue] = color.map(quantize);
        *target = [red, green, blue, 255];
        return;
    }
    for (i, out) in target.iter_mut().enumerate().take(3) {
        *out = quantize(color[i] * alpha + f64::from(*out) / 255.0 * (1.0 - alpha));
    }
    target[3] = quantize(alpha + f64::from(target[3]) / 255.0 * (1.0 - alpha));
}

#[cfg(test)]
mod tests {
    use super::super::{Separations, narrow::Buffer, plan};
    use super::*;
    use crate::RenderOptions;
    use crate::display::DisplayList;
    use crate::transparency::Attributes;
    use pagura_core::{Limits, Rect};
    use pagura_raster::Rgba8;
    use std::sync::{Arc, OnceLock};

    fn context(background: Rgba8) -> Context {
        let list = DisplayList {
            commands: Vec::new(),
            crop: Rect::from_corners(0.0, 0.0, 512.0, 32.0).unwrap(),
            rotation: pagura_document::Rotation::None,
            user_unit: 1.0,
            limits: Limits::default(),
            group: Attributes::default(),
            print_intent: false,
            separations: OnceLock::new(),
        };
        let options = RenderOptions {
            background,
            ..RenderOptions::default()
        };
        let separations = Arc::new(Separations::default());
        let plan = plan::page(&list, &separations).unwrap();
        Context::new(&list, &options, &plan, separations).unwrap()
    }

    #[test]
    fn narrow_page_output_preserves_colored_background_across_chunk_edges() {
        let background = Rgba8 {
            r: 17,
            g: 85,
            b: 170,
            a: 255,
        };
        let rgba = [17, 85, 170, 255];
        let tile = Tile {
            x: 250,
            y: 7,
            width: 12,
            height: 4,
        };
        for format in [plan::Format::N16, plan::Format::U8, plan::Format::F32] {
            for painted in [false, true] {
                let mut context = context(background);
                let mut output = Pixmap::new(12, 4, background, 48).unwrap();
                let before = (context.work, context.allocated);
                for (x, coverage, offset) in [(250, 128, 11), (256, 255, 6)] {
                    let mut buffer = Buffer::new(format, 24, rgba).unwrap();
                    let bytes = match format {
                        plan::Format::U8 => 104,
                        plan::Format::N16 => 192,
                        _ => 384,
                    };
                    assert_eq!(buffer.bytes(), bytes);
                    if painted {
                        buffer.path(offset, &[coverage], [0.0; 4], 1.0).unwrap();
                    }
                    let page = Surface::with_buffer(
                        Tile {
                            x,
                            y: 7,
                            width: 6,
                            height: 4,
                        },
                        buffer,
                    );
                    context
                        .write_output(&page, &[], Model::Rgb, &mut output, tile)
                        .unwrap();
                }
                let mut expected = rgba.repeat(48);
                if painted {
                    // Half-covered black: round([17,85,170] *127/255).
                    // The adjacent fully covered pixel is opaque black.
                    expected[17 * 4..18 * 4].copy_from_slice(&[8, 42, 85, 255]);
                    expected[18 * 4..19 * 4].copy_from_slice(&[0, 0, 0, 255]);
                }
                assert_eq!(output.data(), expected, "{format:?} painted={painted}");
                assert_eq!((context.work, context.allocated), before);
            }
        }
    }

    #[test]
    fn n16_page_output_preserves_holes_in_a_multirow_painted_interval() {
        let background = Rgba8 {
            r: 17,
            g: 85,
            b: 170,
            a: 255,
        };
        let rgba = [17, 85, 170, 255];
        let tile = Tile {
            x: 250,
            y: 7,
            width: 12,
            height: 7,
        };
        let mut context = context(background);
        let mut output = Pixmap::new(12, 7, background, 84).unwrap();
        let before = (context.work, context.allocated);
        let mut buffer = Buffer::new(plan::Format::N16, 30, rgba).unwrap();
        // In one 6-by-5 chunk, the interval starts at column 2 of row 0
        // and ends after column 1 of row 4. Three complete intervening
        // rows and the other pixels in both endpoint rows remain background.
        buffer.path(2, &[128], [0.0; 4], 1.0).unwrap();
        buffer.path(25, &[255], [0.0; 4], 1.0).unwrap();
        assert_eq!(buffer.painted_range(), Some(2..26));
        assert_eq!(buffer.bytes(), 240);
        let page = Surface::with_buffer(
            Tile {
                x: 253,
                y: 8,
                width: 6,
                height: 5,
            },
            buffer,
        );
        context
            .write_output(&page, &[], Model::Rgb, &mut output, tile)
            .unwrap();
        let mut expected = rgba.repeat(84);
        // Device (255,8) maps to output row 1, column 5. Black at
        // coverage 128 leaves round([17,85,170] *127/255).
        expected[17 * 4..18 * 4].copy_from_slice(&[8, 42, 85, 255]);
        // Device (254,12) maps to output row 5, column 4.
        expected[64 * 4..65 * 4].copy_from_slice(&[0, 0, 0, 255]);
        assert_eq!(output.data(), expected);
        assert_eq!((context.work, context.allocated), before);
    }

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
                            *out = quantize(
                                color[i] * alpha + f64::from(*out) / 255.0 * (1.0 - alpha),
                            );
                        }
                        expected[3] =
                            quantize(alpha + f64::from(expected[3]) / 255.0 * (1.0 - alpha));
                    }
                    let mut actual = [background];
                    rgb(&mut actual, &[pixel]);
                    assert_eq!(actual, [expected], "{color} at alpha {alpha}");
                }
            }
        }
    }
}
