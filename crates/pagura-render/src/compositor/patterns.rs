//! Evaluate visible tiling instances as one non-isolated, non-knockout group
//! (8.7.3; 11.6.7). Outer path coverage, alpha and blend are applied once.
use super::{Backdrop, Context, Model, Source, Surface};
use crate::Tile;
use crate::display::PathCommand;
use crate::pattern::Tiling;
use pagura_core::{Error, Result};
use pagura_raster::{Mask, Point, Transform};

impl Context {
    /// Shading patterns first paint their background, then the clipped shading
    /// (11.6.7). Retain geometric boundary coverage for that colour mixture.
    pub(super) fn pattern_boundary(
        &self,
        pattern: Option<&crate::display::ShadingCommand>,
        mask: &Mask,
    ) -> Result<Option<Mask>> {
        let Some(pattern) = pattern else {
            return Ok(None);
        };
        let Some(path) = pattern.shading.bbox_path.as_ref().filter(|path| {
            pattern.shading.background.is_some()
                && path
                    .fill_region(pagura_raster::FillRule::NonZero)
                    .is_some_and(|region| region != pagura_raster::FillRegion::Ordinary)
        }) else {
            return Ok(None);
        };
        Mask::from_path_with_limits(
            mask.width(),
            mask.height(),
            path,
            pattern.transform.then(self.device),
            pagura_raster::FillRule::NonZero,
            self.limits,
            mask.origin(),
        )
        .map(Some)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "explicit source, backdrop and geometry state"
    )]
    pub(super) fn tiling(
        &mut self,
        command: &PathCommand,
        transform: Transform,
        pattern: &Tiling,
        mask: &Mask,
        target: &mut Surface,
        initial: Backdrop<'_>,
        model: Model,
        knockout: bool,
        depth: usize,
    ) -> Result<()> {
        let transform = fit(transform.then(self.device), pattern)?;
        let inverse = transform.inverse().ok_or(Error::unsupported(
            crate::features::PATTERN_TILING,
            "singular tiling transform",
        ))?;
        let tile = mask_tile(mask);
        let b = pattern.bbox;
        let [x0, y0, x1, y1] = cell_envelope(tile, inverse, b);
        let (ix0, ix1) = intervals(x0, x1, b.x0, b.x1, pattern.step[0])?;
        let (iy0, iy1) = intervals(y0, y1, b.y0, b.y1, pattern.step[1])?;
        self.charge(instance_count(
            (ix0, ix1),
            (iy0, iy1),
            self.limits.display_commands,
        )?)?;
        let backdrop = if knockout {
            initial
        } else {
            Backdrop::Surface(target)
        };
        let mut surface = self.surface(tile, backdrop)?;
        let device = self.device;
        let soft_source = self.soft_source;
        self.soft_source = true;
        let result = (|| {
            for y in iy0..=iy1 {
                for x in ix0..=ix1 {
                    self.device = Transform::translate(
                        x as f64 * pattern.step[0],
                        y as f64 * pattern.step[1],
                    )
                    .then(transform);
                    self.clip = None;
                    self.discard_mask();
                    self.commands(
                        &pattern.commands,
                        &mut surface,
                        backdrop,
                        model,
                        false,
                        depth + 1,
                    )?;
                }
            }
            Ok(())
        })();
        self.device = device;
        self.soft_source = soft_source;
        self.clip = None;
        result?;
        let width =
            usize::try_from(tile.width).map_err(|_| Error::limit("pattern width exceeds usize"))?;
        remove_backdrop(&mut surface, backdrop)?;
        let soft = command
            .composite
            .mask
            .as_ref()
            .map(|m| self.soft_mask(m, target.tile, model, depth + 1))
            .transpose()?;
        self.charge(surface.pixels.len())?;
        for (row, pixels) in surface.pixels.chunks_exact(width).enumerate() {
            let y =
                tile.y + u32::try_from(row).map_err(|_| Error::limit("pattern row overflow"))?;
            for (col, pixel) in pixels.iter().enumerate().filter(|(_, p)| p.shape > 0.0) {
                let x = tile.x
                    + u32::try_from(col).map_err(|_| Error::limit("pattern column overflow"))?;
                let coverage = mask.coverage()[row * width + col];
                if coverage == 0 {
                    continue;
                }
                let index = target
                    .index(x, y)
                    .ok_or(Error::invalid_argument("pattern outside target"))?;
                let factor = f64::from(coverage) / 255.0;
                let source = Source {
                    spot: self.spot_batch.is_some(),
                    ..Source::new(pixel.color, pixel.group * factor, pixel.shape * factor)
                }
                .apply(
                    &command.composite,
                    soft.as_ref()
                        .and_then(|m| m.get(index))
                        .copied()
                        .unwrap_or(1.0),
                );
                // All reads of the initial parent pixel precede this pixel's write.
                let base = initial.pixel(x, y);
                target
                    .pixels
                    .get_mut(index)
                    .ok_or(Error::invalid_argument("pattern index outside target"))?
                    .paint(source, &command.composite, base, knockout, model);
            }
        }
        self.allocated -= surface.bytes();
        Ok(())
    }
}

fn envelope(tile: Tile, inverse: Transform) -> [f64; 4] {
    let x = f64::from(tile.x);
    let y = f64::from(tile.y);
    let w = f64::from(tile.width);
    let h = f64::from(tile.height);
    let points =
        [(x, y), (x + w, y), (x, y + h), (x + w, y + h)].map(|(x, y)| Point::new(x, y) * inverse);
    [
        points.iter().map(|p| p.x).fold(f64::INFINITY, f64::min),
        points.iter().map(|p| p.y).fold(f64::INFINITY, f64::min),
        points.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max),
        points.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max),
    ]
}
#[expect(
    clippy::cast_possible_truncation,
    reason = "integral range checked within exact f64 integers"
)]
fn intervals(lo: f64, hi: f64, b0: f64, b1: f64, step: f64) -> Result<(i64, i64)> {
    let start = ((lo - b1) / step).ceil();
    let end = ((hi - b0) / step).floor();
    if !start.is_finite() || !end.is_finite() || start.abs().max(end.abs()) > 4e15 {
        return Err(Error::limit("tiling index exceeds numeric range"));
    }
    Ok((start as i64, end as i64))
}

/// Constant-spacing cells use integer device lattice vectors (Table 74).
/// Type 2 preserves the exact matrix; pixel quantization alone varies spacing
/// by at most one pixel. Type 1 refuses distortion beyond its one-pixel bound.
fn fit(t: Transform, pattern: &Tiling) -> Result<Transform> {
    if pattern.spacing == 2 {
        return Ok(t);
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
    let (a, b) = vector(t.a * pattern.step[0], t.b * pattern.step[0]);
    let (c, d) = vector(t.c * pattern.step[1], t.d * pattern.step[1]);
    let fitted = Transform::new(
        a / pattern.step[0],
        b / pattern.step[0],
        c / pattern.step[1],
        d / pattern.step[1],
        t.e,
        t.f,
    );
    if fitted.inverse().is_none() {
        return Err(Error::unsupported(
            crate::features::PATTERN_TILING,
            "device lattice collapses the pattern",
        ));
    }
    if pattern.spacing == 1 {
        let bbox = pattern.bbox;
        for p in [
            Point::new(bbox.x0, bbox.y0),
            Point::new(bbox.x1, bbox.y0),
            Point::new(bbox.x0, bbox.y1),
            Point::new(bbox.x1, bbox.y1),
        ] {
            let a = p * t;
            let b = p * fitted;
            if (a.x - b.x).hypot(a.y - b.y) > 1.0 + 1e-12 {
                return Err(Error::unsupported(
                    crate::features::PATTERN_TILING,
                    "constant spacing requires excessive cell distortion",
                ));
            }
        }
    }
    Ok(fitted)
}

fn remove_backdrop(surface: &mut Surface, backdrop: Backdrop<'_>) -> Result<()> {
    let tile = surface.tile;
    let width =
        usize::try_from(tile.width).map_err(|_| Error::limit("pattern width exceeds usize"))?;
    for (row, pixels) in surface.pixels.chunks_exact_mut(width).enumerate() {
        let y = tile.y + u32::try_from(row).map_err(|_| Error::limit("pattern row overflow"))?;
        for (col, pixel) in pixels.iter_mut().enumerate().filter(|(_, p)| p.shape > 0.0) {
            let x =
                tile.x + u32::try_from(col).map_err(|_| Error::limit("pattern column overflow"))?;
            let base = backdrop.pixel(x, y);
            pixel.color = if pixel.group > 0.0 {
                [0, 1, 2, 3].map(|i| {
                    ((pixel.color[i] - (1.0 - pixel.group) * base.color[i]) / pixel.group)
                        .clamp(0.0, 1.0)
                })
            } else {
                [0.0; 4]
            };
        }
    }
    Ok(())
}

fn instance_count((x0, x1): (i64, i64), (y0, y1): (i64, i64), maximum: usize) -> Result<usize> {
    (x1 - x0)
        .checked_add(1)
        .and_then(|x| (y1 - y0).checked_add(1).and_then(|y| x.checked_mul(y)))
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n <= maximum)
        .ok_or(Error::limit("tiling instance budget exceeded"))
}

fn mask_tile(mask: &Mask) -> Tile {
    let (x, y) = mask.origin();
    Tile {
        x,
        y,
        width: mask.width(),
        height: mask.height(),
    }
}

fn cell_envelope(tile: Tile, inverse: Transform, b: pagura_core::Rect) -> [f64; 4] {
    let [mut x0, mut y0, mut x1, mut y1] = envelope(tile, inverse);
    if b.is_empty() {
        // Include cells whose one-device-pixel boundary reaches this tile.
        let px = inverse.a.abs() + inverse.c.abs();
        let py = inverse.b.abs() + inverse.d.abs();
        x0 -= px;
        x1 += px;
        y0 -= py;
        y1 += py;
    }
    [x0, y0, x1, y1]
}
