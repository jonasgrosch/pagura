//! Tiling patterns (8.7.3) by exact geometry per instance: every cell
//! instance that can meet the mark is painted with its own translation,
//! together forming one non-isolated, non-knockout group over the mark's
//! backdrop, whose result the mark's coverage, alpha and blend mode then
//! composite once (11.6.7). Constant spacing places the instances a whole
//! number of device pixels apart ([`lattice`]).

use super::transfer::Selection;
use super::{Context, Contribution, Crop, Renderer};
use crate::buffer::{Buffer, Initial};
use crate::color::Channels;
use crate::pixel::Elements;
use crate::shapes::Coverage;
use pagura_core::{Error, Result};
use pagura_interpret::{Composite, Tiling, TilingColors, TilingPaint, TilingSpacing};
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
        let to_device = lattice(tiling.matrix.then(context.device), tiling)?;
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
                    crop: Crop::Free,
                };
                self.children(&cell.children, &mut cells, &inner)?;
            }
        }
        let results = colors(&cells, &backdrop, context.target.channels());
        let contributions = results
            .into_iter()
            .filter_map(|(x, y, color, group, shape)| {
                let factor = coverage.get(x, y);
                (factor > 0.0).then_some(Contribution {
                    x,
                    y,
                    color,
                    alpha: group * factor,
                    shape: shape * factor,
                })
            })
            .collect();
        self.contribute(contributions, state, parent, context)
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
/// The device transform of a tiling pattern's instances (8.7.3.1, Table
/// 74). `TilingType 1` and `3` space cells consistently, "by a multiple of
/// a device pixel": the lattice vectors `XStep·(a, b)` and `YStep·(c, d)`
/// are rounded to whole pixels, a vanishing one to a unit step along its
/// larger component, and the origin kept, distorting the cell slightly.
/// `TilingType 1` admits no cell corner moved by more than a pixel.
/// `TilingType 2` keeps the exact transform.
fn lattice(t: Transform, tiling: &Tiling) -> Result<Transform> {
    if tiling.spacing == TilingSpacing::NoDistortion {
        return Ok(t);
    }
    let whole = |x: f64, y: f64| match (x.round(), y.round()) {
        (0.0, 0.0) if x.abs() >= y.abs() => (x.signum(), 0.0),
        (0.0, 0.0) => (0.0, y.signum()),
        rounded => rounded,
    };
    let [x_step, y_step] = tiling.step;
    let (a, b) = whole(t.a * x_step, t.b * x_step);
    let (c, d) = whole(t.c * y_step, t.d * y_step);
    let fitted = Transform::new(a / x_step, b / x_step, c / y_step, d / y_step, t.e, t.f);
    if fitted.inverse().is_none() {
        return Err(Error::invalid_argument(
            "the reference has no collapsed tiling lattice",
        ));
    }
    if tiling.spacing == TilingSpacing::Constant {
        let b = tiling.bbox;
        for (x, y) in [
            (b.x0(), b.y0()),
            (b.x1(), b.y0()),
            (b.x0(), b.y1()),
            (b.x1(), b.y1()),
        ] {
            let (exact, moved) = (Point::new(x, y) * t, Point::new(x, y) * fitted);
            if (exact.x - moved.x).hypot(exact.y - moved.y) > 1.0 + 1e-12 {
                return Err(Error::invalid_argument(
                    "the reference has no tiling cell distorted by more than a pixel",
                ));
            }
        }
    }
    Ok(fitted)
}

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
