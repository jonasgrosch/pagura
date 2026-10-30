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
        (coverage, geometric): (&Coverage, Option<&[crate::area::Region]>),
        state: &Composite,
        parent: &mut Buffer,
        context: &Context<'_>,
    ) -> Result<()> {
        if geometric.is_none() && !context.inherited.is_empty() {
            return Err(Error::unsupported(
                pagura_interpret::features::PATTERN_IMAGE_MASK,
                "ordinary nested tiling under a patterned stencil is not implemented",
            ));
        }
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
                    inherited: geometric.unwrap_or(context.inherited),
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
                // A stencil is already part of each mark's geometric
                // intersection. Applying its sampled coverage again would
                // square fractional edges and lose mask/ink correlation.
                let factor = geometric.map_or_else(|| coverage.get(x, y), |_| 1.0);
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

/// The device transform that places a tiling pattern's instances, as ADR
/// 0004 section 2 defines it for `TilingType` (8.7.3.1, Table 74): `m`
/// itself for `TilingType 2`; for `TilingType 1` and `3`, the transform
/// with `m`'s origin that takes `(XStep, 0)` and `(0, YStep)` to the device
/// lattice vectors rounded to whole pixels. When the rounded vectors are
/// parallel, or under `TilingType 1` would move a cell corner by more than a
/// pixel, `m` itself, the exact lattice of `TilingType 2`. A singular `m` is
/// refused.
fn lattice(m: Transform, tiling: &Tiling) -> Result<Transform> {
    if m.inverse().is_none() {
        return Err(Error::invalid_argument(
            "the reference has no singular tiling pattern",
        ));
    }
    if tiling.spacing == TilingSpacing::NoDistortion {
        return Ok(m);
    }
    let [x_step, y_step] = tiling.step;
    let (u, v) = ([m.a * x_step, m.b * x_step], [m.c * y_step, m.d * y_step]);
    let (u_whole, v_whole) = (whole_pixels(u), whole_pixels(v));
    if u_whole[0] * v_whole[1] == u_whole[1] * v_whole[0] {
        return Ok(m);
    }
    if tiling.spacing == TilingSpacing::Constant {
        // The two transforms share an origin, so a pattern point `(x, y)`
        // moves by `x/XStep` times the change of `u` plus `y/YStep` times
        // the change of `v`.
        let (du, dv) = (
            [u_whole[0] - u[0], u_whole[1] - u[1]],
            [v_whole[0] - v[0], v_whole[1] - v[1]],
        );
        let b = tiling.bbox;
        for x in [b.x0(), b.x1()] {
            for y in [b.y0(), b.y1()] {
                let (s, t) = (x / x_step, y / y_step);
                if (s * du[0] + t * dv[0]).hypot(s * du[1] + t * dv[1]) > 1.0 + 1e-12 {
                    return Ok(m);
                }
            }
        }
    }
    Ok(Transform::new(
        u_whole[0] / x_step,
        u_whole[1] / x_step,
        v_whole[0] / y_step,
        v_whole[1] / y_step,
        m.e,
        m.f,
    ))
}

/// A device lattice vector in whole pixels: each component rounded to the
/// nearest integer, halves away from zero; a vector rounding to zero
/// becomes one pixel along its component of larger magnitude, the
/// horizontal one on a tie.
fn whole_pixels([x, y]: [f64; 2]) -> [f64; 2] {
    let rounded = [x.round(), y.round()];
    if rounded != [0.0, 0.0] {
        rounded
    } else if x.abs() >= y.abs() {
        [1.0_f64.copysign(x), 0.0]
    } else {
        [0.0, 1.0_f64.copysign(y)]
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::Rect;
    use pagura_interpret::GroupId;

    /// An 8-unit page at 72 dpi, whose device transform flips `y`.
    const PAGE: Transform = Transform::new(1.0, 0.0, 0.0, -1.0, 0.0, 8.0);

    fn place(m: Transform, step: [f64; 2], size: f64, spacing: TilingSpacing) -> Result<Transform> {
        let tiling = Tiling {
            cell: GroupId(0),
            bbox: Rect::from_corners(0.0, 0.0, size, size).unwrap(),
            step,
            matrix: Transform::IDENTITY,
            colors: TilingColors::Colored,
            spacing,
        };
        lattice(m, &tiling)
    }

    /// The device lattice vectors of `l`, to nine decimals.
    fn vectors(l: Transform, [x_step, y_step]: [f64; 2]) -> [f64; 4] {
        [l.a * x_step, l.b * x_step, l.c * y_step, l.d * y_step].map(|v| (v * 1e9).round() / 1e9)
    }

    #[test]
    fn halves_round_away_from_zero() {
        // `(2.5, 0)` and `(0, -2.5)` become `(3, 0)` and `(0, -3)`; rounding
        // halves to even, or up, would give 2 or -2.
        for spacing in [TilingSpacing::Constant, TilingSpacing::Faster] {
            let l = place(PAGE, [2.5, 2.5], 2.5, spacing).unwrap();
            assert_eq!(vectors(l, [2.5, 2.5]), [3.0, 0.0, 0.0, -3.0]);
            assert_eq!((l.e, l.f), (0.0, 8.0));
        }
    }

    #[test]
    fn vanishing_vectors_step_one_pixel_along_their_larger_component() {
        let m = Transform::new(0.4, -0.6, 0.3, 0.3, 0.0, 0.0);
        let l = place(m, [1.0, 1.0], 1.0, TilingSpacing::Faster).unwrap();
        assert_eq!(vectors(l, [1.0, 1.0]), [0.0, -1.0, 1.0, 0.0]);
    }

    #[test]
    fn no_distortion_keeps_the_exact_transform() {
        let l = place(PAGE, [1.6, 1.6], 1.6, TilingSpacing::NoDistortion).unwrap();
        assert_eq!(l, PAGE);
    }

    #[test]
    fn constant_spacing_admits_at_most_a_pixel_of_distortion() {
        // A 1.5-unit step becomes 2 pixels, moving the corner `(s, s)` by
        // `s / 3` pixels along each axis. Beyond a pixel, the exact lattice.
        let corner = |s: f64| s / 3.0 * 2.0_f64.sqrt();
        assert!(corner(2.1) <= 1.0 && corner(2.2) > 1.0);
        let fitted = place(PAGE, [1.5, 1.5], 2.1, TilingSpacing::Constant).unwrap();
        assert_eq!(vectors(fitted, [1.5, 1.5]), [2.0, 0.0, 0.0, -2.0]);
        assert_eq!(
            place(PAGE, [1.5, 1.5], 2.2, TilingSpacing::Constant).unwrap(),
            PAGE
        );
        let faster = place(PAGE, [1.5, 1.5], 2.2, TilingSpacing::Faster).unwrap();
        assert_eq!(vectors(faster, [1.5, 1.5]), [2.0, 0.0, 0.0, -2.0]);
    }

    #[test]
    fn collapsed_lattices_keep_the_exact_transform() {
        // `(0.6, -0.6)` and `(0.6, -0.8)` both round to `(1, -1)`.
        let oblique = Transform::new(0.3, -0.3, 0.3, -0.4, 0.0, 64.0);
        for spacing in [TilingSpacing::Constant, TilingSpacing::Faster] {
            assert_eq!(place(oblique, [2.0, 2.0], 1.0, spacing).unwrap(), oblique);
        }
    }

    #[test]
    fn singular_patterns_are_refused() {
        let singular = Transform::new(0.0, 0.0, 0.0, -1.0, 0.0, 8.0);
        for spacing in [
            TilingSpacing::Constant,
            TilingSpacing::NoDistortion,
            TilingSpacing::Faster,
        ] {
            assert!(place(singular, [2.0, 2.0], 1.0, spacing).is_err());
        }
    }
}
