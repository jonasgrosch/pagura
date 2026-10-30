//! Tiling patterns (8.7.3): the instances meeting a painted mark's pixels
//! as one group, each with its own exact geometry, on the device lattice
//! the pattern's spacing asks for.

use super::{Planner, Scope};
use crate::format::Format;
use crate::plan::{GroupPlan, Start, intersect};
use crate::prepare::{Budget, bounds, index};
use pagura_core::{Error, Result};
use pagura_interpret::{Paint, TilingColors, TilingPaint, TilingSpacing};
use pagura_raster::{DeviceRect, Transform};
use std::sync::Arc;

impl Planner<'_> {
    /// The instances of a tiling pattern meeting `bounds`, as one
    /// non-isolated, non-knockout group composited through the painted
    /// mark's coverage (8.7.3, 11.6.7); every instance has its own exact
    /// geometry.
    pub(crate) fn pattern(
        &mut self,
        scope: &Scope,
        paint: &TilingPaint,
        bounds: DeviceRect,
        budget: &mut Budget,
    ) -> Result<Option<(u32, DeviceRect)>> {
        let scene = self.scene;
        let tiling = scene
            .tilings
            .get(index(paint.pattern.0))
            .ok_or(Error::invalid_argument("scene pattern out of range"))?;
        let cell = scene
            .group(tiling.cell)
            .ok_or(Error::invalid_argument("scene group out of range"))?;
        let transform = fit(tiling.matrix.then(scope.transform), tiling);
        let inverse = transform.inverse().ok_or(Error::unsupported(
            pagura_interpret::features::PATTERN_TILING,
            "singular tiling transform",
        ))?;
        let (columns, rows) = instances(bounds, inverse, tiling)?;
        let count = (columns.1 - columns.0 + 1).saturating_mul(rows.1 - rows.0 + 1);
        if usize::try_from(count).map_or(true, |n| n > self.limits.display_commands) {
            return Err(Error::limit("tiling instance budget exceeded"));
        }
        let override_paint = match tiling.colors {
            TilingColors::Colored => None,
            TilingColors::Uncolored => Some(self.pattern_paint(scope, paint, bounds, budget)?),
        };
        let temporary = override_paint.as_ref().map_or(0, |(_, paint)| {
            let Paint::Solid(color) = &**paint else {
                unreachable!("pattern override is solid")
            };
            2 * size_of::<usize>() + size_of::<Paint>() + color.components.heap_bytes()
        });
        let prepared = (|| {
            let transparent = self
                .storage
                .cells
                .get(&(paint.pattern, scope.layout.process))
                .copied()
                .unwrap_or(false);
            let group = self.new_group(
                GroupPlan {
                    bounds,
                    items: Vec::new(),
                    order: Vec::new(),
                    layout: scope.layout,
                    format: Format::F64,
                    knockout: false,
                    start: if transparent {
                        Start::Transparent
                    } else {
                        Start::Backdrop
                    },
                },
                budget,
            )?;
            let [x_step, y_step] = tiling.step;
            for j in rows.0..=rows.1 {
                for i in columns.0..=columns.1 {
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "instance indices are below 2^53"
                    )]
                    let offset = Transform::translate(i as f64 * x_step, j as f64 * y_step);
                    let inner = Scope {
                        group,
                        transform: offset.then(transform),
                        crop: false,
                        bounds,
                        layout: scope.layout,
                        plain: false,
                        records: false,
                        separated: scope.separated,
                        masked: scope.masked,
                        paint: override_paint.clone().or_else(|| scope.paint.clone()),
                        depth: scope.depth + 1,
                    };
                    self.children(&inner, &cell.children, budget)?;
                }
            }
            let content = intersect(self.content(group), bounds);
            if let Some(plan) = self.parts.groups.get_mut(index(group)) {
                plan.bounds = content;
            }
            Ok((!content.is_empty()).then_some((group, content)))
        })();
        drop(override_paint);
        budget.bytes.refund(temporary);
        prepared
    }

    /// One temporary, shared colour override for an uncoloured cell.
    fn pattern_paint(
        &mut self,
        scope: &Scope,
        tiling: &TilingPaint,
        bounds: DeviceRect,
        budget: &mut Budget,
    ) -> Result<(u32, Arc<Paint>)> {
        let color = tiling
            .color
            .as_ref()
            .ok_or(Error::syntax("uncoloured pattern without a colour"))?;
        budget
            .bytes
            .charge(2 * size_of::<usize>() + size_of::<Paint>() + color.components.heap_bytes())?;
        let state = pagura_interpret::Composite::default();
        let paint = Arc::new(Paint::Solid(color.clone()));
        let prepared = self.solid_paint(&paint, (&state, scope), bounds, budget);
        if let Err(error) = prepared {
            let released =
                2 * size_of::<usize>() + size_of::<Paint>() + color.components.heap_bytes();
            drop(paint);
            budget.bytes.refund(released);
            return Err(error);
        }
        Ok((prepared?, paint))
    }
}

/// The instance index ranges of a tiling pattern whose cells can meet
/// `bounds`: cells of `BBox` at multiples of `XStep` and `YStep`.
fn instances(
    bounds: DeviceRect,
    inverse: Transform,
    tiling: &pagura_interpret::Tiling,
) -> Result<((i64, i64), (i64, i64))> {
    let corners = [
        (bounds.x0, bounds.y0),
        (bounds.x1, bounds.y0),
        (bounds.x0, bounds.y1),
        (bounds.x1, bounds.y1),
    ]
    .map(|(x, y)| inverse.apply(pagura_raster::Point::new(f64::from(x), f64::from(y))));
    let mut hull = bounds::points(corners).unwrap_or([0.0; 4]);
    let b = tiling.bbox;
    if b.is_empty() {
        // Cells whose one-pixel boundary reaches the bounds.
        let px = inverse.a.abs() + inverse.c.abs();
        let py = inverse.b.abs() + inverse.d.abs();
        hull = [hull[0] - px, hull[1] - py, hull[2] + px, hull[3] + py];
    }
    let columns = range(hull[0], hull[2], (b.x0(), b.x1()), tiling.step[0])?;
    let rows = range(hull[1], hull[3], (b.y0(), b.y1()), tiling.step[1])?;
    Ok((columns, rows))
}

/// The device transform of a tiling pattern's cells (Table 74): constant
/// spacing rounds the lattice vectors `XStep·(a, b)` and `YStep·(c, d)` to
/// whole pixels, a vanishing one to a unit step along its larger component.
/// `TilingType 2` keeps the exact transform, as do a singular one, which has
/// no lattice to fit, rounded vectors that are parallel, and under
/// `TilingType 1` a cell corner the rounding would move by more than a pixel
/// (ADR 0004 section 2).
fn fit(t: Transform, tiling: &pagura_interpret::Tiling) -> Transform {
    if tiling.spacing == TilingSpacing::NoDistortion || t.inverse().is_none() {
        return t;
    }
    let vector = |x: f64, y: f64| {
        let (mut a, mut b) = (x.round(), y.round());
        if a == 0.0 && b == 0.0 {
            if x.abs() >= y.abs() {
                a = x.signum();
            } else {
                b = y.signum();
            }
        }
        (a, b)
    };
    let [x_step, y_step] = tiling.step;
    let (a, b) = vector(t.a * x_step, t.b * x_step);
    let (c, d) = vector(t.c * y_step, t.d * y_step);
    let fitted = Transform::new(a / x_step, b / x_step, c / y_step, d / y_step, t.e, t.f);
    // Without a whole-pixel lattice that serves, the exact TilingType 2
    // lattice places the cells (ADR 0004 section 2).
    if fitted.inverse().is_none() {
        return t;
    }
    if tiling.spacing == TilingSpacing::Constant {
        let b = tiling.bbox;
        for (x, y) in [
            (b.x0(), b.y0()),
            (b.x1(), b.y0()),
            (b.x0(), b.y1()),
            (b.x1(), b.y1()),
        ] {
            let p = pagura_raster::Point::new(x, y);
            let (exact, moved) = (t.apply(p), fitted.apply(p));
            if (exact.x - moved.x).hypot(exact.y - moved.y) > 1.0 + 1e-12 {
                return t;
            }
        }
    }
    fitted
}

/// The integers `k` for which `[b0, b1] + k·step` meets `[lo, hi]`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "integral values range-checked within exact f64 integers"
)]
fn range(lo: f64, hi: f64, (b0, b1): (f64, f64), step: f64) -> Result<(i64, i64)> {
    let a = (lo - b1) / step;
    let b = (hi - b0) / step;
    let start = a.min(b).ceil();
    let end = a.max(b).floor();
    if !start.is_finite() || !end.is_finite() || start.abs().max(end.abs()) > 4e15 {
        return Err(Error::limit("tiling index exceeds numeric range"));
    }
    Ok((start as i64, (end as i64).max(start as i64 - 1)))
}
