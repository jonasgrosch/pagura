//! Output (11.3.3): the page's colour converted to RGB, with simulated spot
//! inks and the selected transfer functions, composited onto the
//! background, which is premultiplied once to bytes as the output pixmap
//! holds it.

use super::Renderer;
use crate::buffer::Buffer;
use crate::color::{MAX, Prepared, components};
use crate::overprint::Definition;
use crate::request::Rendering;
use pagura_color::{Conversion, Process, Target};
use pagura_core::Result;
use pagura_raster::Rgba8;
use std::rc::Rc;
use std::sync::Arc;

impl Renderer<'_> {
    /// The exact output of the page buffer.
    pub(super) fn output(&mut self, page: &Buffer, target: &Target) -> Result<Rendering> {
        let display = self.display(target)?;
        let back = premultiplied(self.request.background);
        let window = self.window;
        let mut pixels = Vec::with_capacity(crate::area::pixels(window));
        for y in window.y0..window.y1 {
            for x in window.x0..window.x1 {
                let pixel = page.get(x, y);
                let alpha = pixel.alpha;
                if alpha <= 0.0 {
                    pixels.push(back);
                    continue;
                }
                let mut rgb = display.rgb(target, &pixel.straight(target.channels()));
                if let Some(transfers) = &self.transfers {
                    rgb = transfers.apply((x, y), rgb);
                }
                let c = rgb.map(|v| v.clamp(0.0, 1.0));
                pixels.push([
                    c[0] * alpha + back[0] * (1.0 - alpha),
                    c[1] * alpha + back[1] * (1.0 - alpha),
                    c[2] * alpha + back[2] * (1.0 - alpha),
                    alpha + back[3] * (1.0 - alpha),
                ]);
            }
        }
        Ok(Rendering::new(window, pixels))
    }

    /// The conversions from the page's blending space to the display.
    fn display(&mut self, target: &Target) -> Result<Display> {
        let conversion = Conversion::default();
        let device = |process| Target {
            process,
            spots: Arc::from([]),
        };
        let (rgb, cmyk) = (device(Process::Rgb), device(Process::Cmyk));
        let to_rgb = self
            .colors
            .prepare_process(target.process, &rgb, &conversion)?;
        if target.spots.is_empty() {
            return Ok(Display {
                to_rgb,
                preview: None,
            });
        }
        let to_cmyk = self
            .colors
            .prepare_process(target.process, &cmyk, &conversion)?;
        let cmyk_to_rgb = self
            .colors
            .prepare_process(Process::Cmyk, &rgb, &conversion)?;
        let mut definitions = Vec::new();
        for definition in &self.simulation.definitions.clone() {
            let convert = self.colors.prepare(&definition.space, &cmyk, &conversion)?;
            definitions.push((definition.clone(), convert));
        }
        Ok(Display {
            to_rgb,
            preview: Some(Preview {
                to_cmyk,
                cmyk_to_rgb,
                definitions,
            }),
        })
    }
}

/// The background premultiplied to bytes once (as the output pixmap holds
/// it), in `0..=1`.
fn premultiplied(background: Rgba8) -> [f64; 4] {
    let alpha = u32::from(background.a);
    let premultiply = |c: u8| f64::from((u32::from(c) * alpha + 127) / 255) / 255.0;
    [
        premultiply(background.r),
        premultiply(background.g),
        premultiply(background.b),
        f64::from(background.a) / 255.0,
    ]
}

/// How page colours become display RGB.
struct Display {
    to_rgb: Rc<Prepared>,
    preview: Option<Preview>,
}

/// Simulated spot inks: process inks plus each spot's CMYK equivalent.
struct Preview {
    to_cmyk: Rc<Prepared>,
    cmyk_to_rgb: Rc<Prepared>,
    definitions: Vec<(Definition, Rc<Prepared>)>,
}

impl Display {
    /// The RGB of a straight page colour.
    fn rgb(&self, target: &Target, straight: &[f64]) -> [f64; 3] {
        let process = target.process.components();
        let mut color = [0.0; MAX];
        let Some(preview) = &self.preview else {
            self.to_rgb.convert(
                &components(target.process, &straight[..process]),
                &mut color[..3],
            );
            return [color[0], color[1], color[2]];
        };
        let mut inks = [0.0; MAX];
        preview.to_cmyk.convert(
            &components(target.process, &straight[..process]),
            &mut inks[..4],
        );
        for (definition, convert) in &preview.definitions {
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
        preview
            .cmyk_to_rgb
            .convert(&components(Process::Cmyk, &inks[..4]), &mut color[..3]);
        [color[0], color[1], color[2]]
    }
}
