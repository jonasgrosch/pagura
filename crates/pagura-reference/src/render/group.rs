//! Transparency groups (11.4, 11.6.6) and soft masks (11.6.5).

use super::transfer::{Selection, simple};
use super::{Context, Renderer, elements, transparency};
use crate::buffer::{Buffer, Initial};
use crate::color::{Channels, MAX, components, luminosity};
use crate::pixel::{Blending, Elements, Pixel, Source};
use pagura_color::{Process, Target};
use pagura_core::{Error, Result, ResultExt};
use pagura_interpret::{GroupId, MaskId, MaskKind, ViewFit};
use pagura_raster::Transform;
use std::sync::Arc;

/// A group's result at one pixel: its colour without the backdrop, and the
/// pixel's group alpha and shape.
type GroupResult = (u32, u32, Channels, Pixel);

impl Renderer<'_> {
    /// Paints group `id` where it occurs: its content in a buffer of its
    /// own, from its initial backdrop, then its colour with that backdrop
    /// removed, its group alpha and its shape, composited into `parent`
    /// with the group's state (11.4.4).
    pub(super) fn group(
        &mut self,
        id: GroupId,
        parent: &mut Buffer,
        context: &Context<'_>,
    ) -> Result<()> {
        let group = self
            .scene
            .group(id)
            .ok_or(Error::invalid_argument("group id outside the scene"))?;
        if !transparency(group.role) {
            return Err(Error::invalid_argument(
                "only transparency groups paint where they occur",
            ));
        }
        let state = self.state(&group.mark)?;
        let attributes = group.attributes;
        let device = match group.view {
            Some(view) => self.view(view, context.device)?,
            None => context.device,
        };
        let process = if attributes.isolated {
            attributes.space.unwrap_or(context.target.process)
        } else {
            context.target.process
        };
        let target = Target {
            process,
            spots: Arc::clone(&context.target.spots),
        };
        let initial = if attributes.isolated {
            Initial::Transparent
        } else if context.elements == Elements::Knockout {
            context.initial
        } else {
            Initial::Buffer(parent)
        };
        let mut buffer = Buffer::new(
            parent.rect(),
            target.channels(),
            &initial,
            self.limits.pixels,
        )?;
        let selection = match context.selection {
            Selection::Opaque if simple(state) => Selection::Opaque,
            Selection::Off => Selection::Off,
            Selection::Opaque | Selection::Translucent => Selection::Translucent,
        };
        let inner = Context {
            device,
            target: &target,
            elements: elements(attributes.knockout),
            initial,
            selection,
            stencil: context.stencil,
            depth: context.depth + 1,
        };
        self.children(&group.children, &mut buffer, &inner)
            .at(group.mark.position)?;
        let results = self.group_colors(&buffer, &initial, &target, context.target, state)?;
        let mask = state
            .soft_mask
            .map(|mask| self.soft_mask(mask, context.device, context.target.process, context.depth))
            .transpose()?;
        let blending = Blending {
            mode: state.blend,
            process: context.target.process,
            channels: context.target.channels(),
        };
        for (x, y, color, pixel) in results {
            let mut source = Source {
                color,
                alpha: pixel.group,
                shape: pixel.shape,
                preserve: 0,
            };
            let m = mask
                .as_ref()
                .and_then(|m| crate::area::index(self.window, x, y).and_then(|i| m.get(i)))
                .copied()
                .unwrap_or(1.0);
            source.apply(state, m);
            let base = context.initial.pixel(x, y);
            let mut out = parent.get(x, y);
            out.paint(&source, blending, &base, context.elements);
            parent.set(x, y, &out);
        }
        Ok(())
    }

    /// The group's colour at every pixel it shapes, with its initial
    /// backdrop removed (11.4.4) and converted to the parent's blending
    /// space (11.7.2); spot channels carry over.
    fn group_colors(
        &mut self,
        buffer: &Buffer,
        initial: &Initial<'_>,
        target: &Target,
        parent: &Target,
        state: &pagura_interpret::Composite,
    ) -> Result<Vec<GroupResult>> {
        let rect = buffer.rect();
        let n = target.channels();
        let convert = (target.process != parent.process)
            .then(|| {
                let process_only = Target {
                    process: parent.process,
                    spots: Arc::from([]),
                };
                self.colors
                    .prepare_process(target.process, &process_only, &state.conversion)
            })
            .transpose()?;
        let mut results = Vec::new();
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let pixel = buffer.get(x, y);
                if pixel.shape <= 0.0 {
                    continue;
                }
                let mut color = pixel.group_color(&initial.pixel(x, y), n);
                if let Some(convert) = &convert {
                    let from = target.process.components();
                    let to = parent.process.components();
                    let mut converted = [0.0; MAX];
                    convert.convert(
                        &components(target.process, &color[..from]),
                        &mut converted[..to],
                    );
                    let spots = n - from;
                    converted[to..to + spots].copy_from_slice(&color[from..n]);
                    color = converted;
                }
                results.push((x, y, color, pixel));
            }
        }
        Ok(results)
    }

    /// The values of soft mask `id` over the window (11.6.5.2): its group
    /// painted from transparency, or for a luminosity mask from its
    /// backdrop colour, then its group alpha or luminosity, then its
    /// transfer function.
    pub(super) fn mask_values(
        &mut self,
        id: MaskId,
        device: Transform,
        parent: Process,
        depth: usize,
    ) -> Result<Vec<f64>> {
        let mask = self
            .scene
            .masks
            .get(pagura_core::convert::u32_to_usize(id.0))
            .ok_or(Error::invalid_argument("soft mask id outside the scene"))?;
        let group = self
            .scene
            .group(mask.group)
            .ok_or(Error::invalid_argument("group id outside the scene"))?;
        let process = group.attributes.space.unwrap_or(parent);
        let target = Target {
            process,
            spots: Arc::from([]),
        };
        let n = process.components();
        let mut backdrop = [0.0; 4];
        match &mask.backdrop {
            Some(values) => {
                for (out, &v) in backdrop.iter_mut().zip(values.iter()).take(n) {
                    let v = v.clamp(0.0, 1.0);
                    *out = if process == Process::Cmyk { 1.0 - v } else { v };
                }
            }
            None if process == Process::Cmyk => backdrop = [1.0, 1.0, 1.0, 0.0],
            None => {}
        }
        let isolated = group.attributes.isolated;
        let initial = if mask.kind == MaskKind::Luminosity && !isolated {
            Initial::Color(&backdrop[..n])
        } else {
            Initial::Transparent
        };
        let mut buffer = Buffer::new(self.window, n, &initial, self.limits.pixels)?;
        let context = Context {
            device,
            target: &target,
            elements: elements(group.attributes.knockout),
            initial,
            selection: Selection::Off,
            stencil: None,
            depth: depth + 1,
        };
        self.children(&group.children, &mut buffer, &context)
            .at(mask.position)?;
        let mut values = Vec::with_capacity(crate::area::pixels(self.window));
        let window = self.window;
        for y in window.y0..window.y1 {
            for x in window.x0..window.x1 {
                let pixel = buffer.get(x, y);
                let value = match mask.kind {
                    MaskKind::Alpha => pixel.group,
                    MaskKind::Luminosity => {
                        let mut color = pixel.color;
                        if isolated {
                            for i in 0..n {
                                color[i] += (1.0 - pixel.alpha) * backdrop[i];
                            }
                        }
                        luminosity(process, &color[..n])
                    }
                }
                .clamp(0.0, 1.0);
                let value = match &mask.transfer {
                    Some(function) => {
                        let mut out = [0.0];
                        function.evaluate(&[value], &mut out);
                        out[0].clamp(0.0, 1.0)
                    }
                    None => value,
                };
                values.push(value);
            }
        }
        Ok(values)
    }

    /// The device transform of an annotation's fixed view (12.5.3): about
    /// its anchor, without the page's rotation (`NoRotate`) or scale
    /// (`NoZoom`).
    fn view(&self, view: ViewFit, device: Transform) -> Result<Transform> {
        let anchor = view.anchor * device;
        let angle = if view.no_rotate {
            -device.b.atan2(device.a)
        } else {
            0.0
        };
        let scale = if view.no_zoom {
            self.scene.page.user_unit / device.a.hypot(device.b)
        } else {
            1.0
        };
        let (sin, cos) = angle.sin_cos();
        let fitted = device
            .then(Transform::translate(-anchor.x, -anchor.y))
            .then(Transform::new(
                scale * cos,
                scale * sin,
                -scale * sin,
                scale * cos,
                anchor.x,
                anchor.y,
            ));
        if !fitted.is_finite() {
            return Err(Error::limit("annotation view transform overflow"));
        }
        Ok(fitted)
    }
}
