//! Tiling patterns (8.7.3) by exact geometry per instance: every cell
//! instance that can meet the mark is painted with its own translation,
//! together forming one non-isolated, non-knockout group over the mark's
//! backdrop, whose result the mark's coverage, alpha and blend mode then
//! composite once (11.6.7).

use super::transfer::Selection;
use super::{Context, Renderer};
use crate::buffer::{Buffer, Initial};
use crate::color::Channels;
use crate::pixel::{Blending, Elements, Source};
use crate::shapes::Coverage;
use pagura_core::{Error, Result};
use pagura_interpret::{Composite, TilingColors, TilingPaint};
use pagura_raster::{Point, Transform};

impl Renderer<'_> {
    /// Paints a tiling pattern over `coverage`.
    pub(super) fn tiling(
        &mut self,
        paint: &TilingPaint,
        coverage: &Coverage,
        state: &Composite,
        parent: &mut Buffer,
        context: &Context<'_>,
    ) -> Result<()> {
        let tiling = self
            .scene
            .tilings
            .get(pagura_core::convert::u32_to_usize(paint.pattern.0))
            .ok_or(Error::invalid_argument("pattern id outside the scene"))?;
        let cell = self
            .scene
            .group(tiling.cell)
            .ok_or(Error::invalid_argument("group id outside the scene"))?;
        let rect = coverage.rect();
        if rect.is_empty() {
            return Ok(());
        }
        let to_device = tiling.matrix.then(context.device);
        let inverse = to_device.inverse().ok_or(Error::invalid_argument(
            "the reference has no singular tiling pattern",
        ))?;
        let [x0, y0, x1, y1] = envelope(rect, inverse);
        let b = tiling.bbox;
        let (columns, rows) = (
            instances(x0, x1, [b.x0(), b.x1()], tiling.step[0])?,
            instances(y0, y1, [b.y0(), b.y1()], tiling.step[1])?,
        );
        let count = (columns.1 - columns.0 + 1).saturating_mul(rows.1 - rows.0 + 1);
        if usize::try_from(count).map_or(true, |n| n > self.limits.display_commands) {
            return Err(Error::limit("tiling instance budget exceeded"));
        }
        let backdrop = if context.elements == Elements::Knockout {
            context.initial
        } else {
            Initial::Buffer(parent)
        };
        let mut cells = Buffer::new(
            rect,
            context.target.channels(),
            &backdrop,
            self.limits.pixels,
        )?;
        let stencil = match tiling.colors {
            TilingColors::Colored => None,
            TilingColors::Uncolored => Some(paint.color.as_ref().ok_or(
                Error::invalid_argument("uncoloured pattern without a colour"),
            )?),
        };
        for row in rows.0..=rows.1 {
            for column in columns.0..=columns.1 {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "instance indices are below 2^52"
                )]
                let offset = Transform::translate(
                    column as f64 * tiling.step[0],
                    row as f64 * tiling.step[1],
                );
                let inner = Context {
                    device: offset.then(to_device),
                    target: context.target,
                    elements: Elements::Accumulate,
                    initial: backdrop,
                    selection: Selection::Off,
                    stencil,
                    depth: context.depth + 1,
                };
                self.children(&cell.children, &mut cells, &inner)?;
            }
        }
        let results = colors(&cells, &backdrop, context.target.channels());
        let mask = state
            .soft_mask
            .map(|mask| self.soft_mask(mask, context.device, context.target.process, context.depth))
            .transpose()?;
        let blending = Blending {
            mode: state.blend,
            process: context.target.process,
            channels: context.target.channels(),
        };
        for (x, y, color, group, shape) in results {
            let covered = coverage.get(x, y);
            if covered == 0 {
                continue;
            }
            let factor = f64::from(covered) / 255.0;
            let mut source = Source {
                color,
                alpha: group * factor,
                shape: shape * factor,
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
}

/// The colour of the instances with the backdrop removed, with group alpha
/// and shape, at each pixel they shape.
fn colors(cells: &Buffer, backdrop: &Initial<'_>, n: usize) -> Vec<(u32, u32, Channels, f64, f64)> {
    let rect = cells.rect();
    let mut results = Vec::new();
    for y in rect.y0..rect.y1 {
        for x in rect.x0..rect.x1 {
            let pixel = cells.get(x, y);
            if pixel.shape <= 0.0 {
                continue;
            }
            let color = pixel.group_color(&backdrop.pixel(x, y), n);
            results.push((x, y, color, pixel.group, pixel.shape));
        }
    }
    results
}

/// The pattern-space bounding box of a device rectangle.
fn envelope(rect: pagura_raster::DeviceRect, inverse: Transform) -> [f64; 4] {
    let (x0, y0, x1, y1) = (
        f64::from(rect.x0),
        f64::from(rect.y0),
        f64::from(rect.x1),
        f64::from(rect.y1),
    );
    let corners = [(x0, y0), (x1, y0), (x0, y1), (x1, y1)].map(|(x, y)| Point::new(x, y) * inverse);
    corners.iter().fold(
        [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ],
        |b, p| [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)],
    )
}

/// The instance indices `k` whose cell `[b0, b1] + k step` can meet
/// `[low, high]`, widened by one so that hairline outlines of empty cells
/// reach.
fn instances(low: f64, high: f64, [b0, b1]: [f64; 2], step: f64) -> Result<(i64, i64)> {
    if !(step.is_finite() && step != 0.0) {
        return Err(Error::invalid_argument(
            "the reference has no tiling step of zero",
        ));
    }
    let (a, b) = ((low - b1) / step, (high - b0) / step);
    let (start, end) = (a.min(b).floor() - 1.0, a.max(b).ceil() + 1.0);
    if !(start.is_finite() && end.is_finite()) || start.abs().max(end.abs()) > 4e15 {
        return Err(Error::limit("tiling index exceeds numeric range"));
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "integral values within ±4e15"
    )]
    Ok((start as i64, end as i64))
}
