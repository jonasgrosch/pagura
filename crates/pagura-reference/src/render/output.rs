//! Output (11.3.3): the page's colour converted to RGB, with simulated spot
//! inks and the selected transfer functions, composited onto the
//! background, which is premultiplied once to bytes as the output pixmap
//! holds it.

use super::Renderer;
use crate::buffer::Buffer;
use crate::color::{MAX, components};
use crate::request::Rendering;
use pagura_color::{Conversion, Process, Target};
use pagura_core::Result;
use std::sync::Arc;

impl Renderer<'_> {
    /// The exact output of the page buffer.
    pub(super) fn output(&mut self, page: &Buffer, target: &Target) -> Result<Rendering> {
        let conversion = Conversion::default();
        let rgb = Target {
            process: Process::Rgb,
            spots: Arc::from([]),
        };
        let cmyk = Target {
            process: Process::Cmyk,
            spots: Arc::from([]),
        };
        let to_rgb = self
            .colors
            .prepare_process(target.process, &rgb, &conversion)?;
        let preview = if target.spots.is_empty() {
            None
        } else {
            let to_cmyk = self
                .colors
                .prepare_process(target.process, &cmyk, &conversion)?;
            let cmyk_to_rgb = self
                .colors
                .prepare_process(Process::Cmyk, &rgb, &conversion)?;
            let mut definitions = Vec::new();
            for definition in &self.simulation.definitions {
                let convert = self.colors.prepare(&definition.space, &cmyk, &conversion)?;
                definitions.push((definition, convert));
            }
            Some((to_cmyk, cmyk_to_rgb, definitions))
        };
        let background = self.request.background;
        let alpha_byte = u32::from(background.a);
        let premultiply = |c: u8| f64::from((u32::from(c) * alpha_byte + 127) / 255) / 255.0;
        let back = [
            premultiply(background.r),
            premultiply(background.g),
            premultiply(background.b),
            f64::from(background.a) / 255.0,
        ];
        let window = self.window;
        let process = target.process.components();
        let mut pixels = Vec::with_capacity(crate::area::pixels(window));
        for y in window.y0..window.y1 {
            for x in window.x0..window.x1 {
                let pixel = page.get(x, y);
                let alpha = pixel.alpha;
                if alpha <= 0.0 {
                    pixels.push(back);
                    continue;
                }
                let straight = pixel.straight(target.channels());
                let mut color = [0.0; MAX];
                match &preview {
                    None => {
                        to_rgb.convert(
                            &components(target.process, &straight[..process]),
                            &mut color[..3],
                        );
                    }
                    Some((to_cmyk, cmyk_to_rgb, definitions)) => {
                        let mut inks = [0.0; MAX];
                        to_cmyk.convert(
                            &components(target.process, &straight[..process]),
                            &mut inks[..4],
                        );
                        for (definition, convert) in definitions {
                            let mut tints = [0.0; pagura_color::MAX_COMPONENTS];
                            for &(input, spot) in &definition.inputs {
                                if let (Some(tint), Some(&value)) =
                                    (tints.get_mut(input), straight.get(process + spot))
                                {
                                    *tint = 1.0 - value;
                                }
                            }
                            if tints.iter().all(|&t| t == 0.0) {
                                continue;
                            }
                            let mut equivalent = [0.0; MAX];
                            convert.convert(
                                &tints[..definition.space.components()],
                                &mut equivalent[..4],
                            );
                            for (ink, e) in inks.iter_mut().zip(equivalent).take(4) {
                                *ink = (*ink + e - 1.0).max(0.0);
                            }
                        }
                        cmyk_to_rgb
                            .convert(&components(Process::Cmyk, &inks[..4]), &mut color[..3]);
                    }
                }
                let mut rgb = [color[0], color[1], color[2]];
                if let Some(transfers) = &self.transfers {
                    rgb = transfers.apply((x, y), rgb);
                }
                pixels.push([
                    rgb[0].clamp(0.0, 1.0) * alpha + back[0] * (1.0 - alpha),
                    rgb[1].clamp(0.0, 1.0) * alpha + back[1] * (1.0 - alpha),
                    rgb[2].clamp(0.0, 1.0) * alpha + back[2] * (1.0 - alpha),
                    alpha + back[3] * (1.0 - alpha),
                ]);
            }
        }
        Ok(Rendering::new(window, pixels))
    }
}
