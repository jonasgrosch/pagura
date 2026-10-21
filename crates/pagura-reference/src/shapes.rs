//! The device regions of marks and clips, and their coverage bytes.
//!
//! A mark's coverage of a pixel is the exact area of the intersection of
//! its shape with every clipping path of its clip, quantized once with
//! [`quantize_unit`], as the renderer's coverage bytes are (ADR 0034).
//! Strokes take `pagura-raster`'s stroke outlines, since the contract
//! concerns the outline's area; empty fills and clips take its hairline
//! outline (10.7.4). Nothing of its coverage code is used.

use crate::area::{Region, areas, index, pixels};
use crate::flatten::{glyph_region, path_region};
use pagura_core::{Error, Limits, Rect, Result};
use pagura_interpret::{ClipId, GlyphSource, Pen, Scene};
use pagura_raster::{
    Adjustment, DeviceRect, FillRegion, FillRule, Path, Point, Transform, adjusted_stroke_outline,
    empty_fill_outline, quantize_unit, stroke_outline,
};

/// The region of a fill or clipping path; an empty filling region with a
/// boundary becomes its hairline outline (10.7.4).
///
/// # Errors
///
/// Returns the geometry's `InvalidArgument` and `Limit` errors.
pub(crate) fn fill(
    path: &Path,
    rule: FillRule,
    device: Transform,
    limits: &Limits,
) -> Result<Region> {
    let mut classified = path.clone();
    let mut work = 0;
    classified.classify_fill(rule, limits, &mut work)?;
    if classified.fill_region(rule) == Some(FillRegion::EmptyBoundary)
        && let Some(outline) = empty_fill_outline(&classified, device, rule, limits)?
    {
        return path_region(&outline, Transform::IDENTITY, FillRule::NonZero, limits);
    }
    path_region(path, device, rule, limits)
}

/// The region of a stroke of `path` (default user space) with `pen`.
///
/// # Errors
///
/// Returns `InvalidArgument` for a singular pen and the stroker's errors.
pub(crate) fn stroke(path: &Path, pen: &Pen, device: Transform, limits: &Limits) -> Result<Region> {
    let inverse = pen.transform.inverse().ok_or(Error::invalid_argument(
        "the reference has no stroke for a singular pen",
    ))?;
    let local = path.transformed(inverse);
    let transform = pen.transform.then(device);
    let (outline, outline_transform) = match pen.adjustment {
        Adjustment::None => stroke_outline(&local, transform, &pen.stroke, limits)?,
        Adjustment::Adjusted => adjusted_stroke_outline(&local, transform, &pen.stroke, limits)?,
    };
    path_region(&outline, outline_transform, FillRule::NonZero, limits)
}

/// The regions of a clip chain, innermost first; empty without a clip.
///
/// # Errors
///
/// As [`fill`], and `InvalidArgument` for a clip id the scene lacks.
pub(crate) fn clips(
    scene: &Scene,
    clip: Option<ClipId>,
    device: Transform,
    limits: &Limits,
) -> Result<Vec<Region>> {
    let mut regions = Vec::new();
    let mut current = clip;
    while let Some(id) = current {
        let node = scene
            .clips
            .get(id.index())
            .ok_or(Error::invalid_argument("clip id outside the scene"))?;
        if regions.len() >= limits.mask_sources.max(1) * 64 {
            return Err(Error::limit("clip chain too long for the reference"));
        }
        regions.push(fill(&node.path, node.rule, device, limits)?);
        current = node.parent;
    }
    Ok(regions)
}

/// The region of one glyph placement at `origin` (default user space, or
/// pattern space in a tiling cell).
///
/// # Errors
///
/// As [`fill`].
pub(crate) fn glyph(
    source: &GlyphSource,
    origin: Point,
    device: Transform,
    limits: &Limits,
) -> Result<Region> {
    let linear = source.linear.then(Transform {
        e: 0.0,
        f: 0.0,
        ..device
    });
    glyph_region(&source.outline, linear, origin * device, limits)
}

/// The region of a rectangle under `transform`, as a fill.
///
/// # Errors
///
/// As [`fill`].
pub(crate) fn rect(rect: Rect, transform: Transform, limits: &Limits) -> Result<Region> {
    fill(&Path::rect(rect), FillRule::NonZero, transform, limits)
}

/// The unit square under `transform`: an image's parallelogram.
///
/// # Errors
///
/// As [`fill`].
pub(crate) fn unit_square(transform: Transform, limits: &Limits) -> Result<Region> {
    let square =
        Rect::from_corners(0.0, 0.0, 1.0, 1.0).ok_or(Error::invalid_argument("unit square"))?;
    path_region(&Path::rect(square), transform, FillRule::NonZero, limits)
}

/// Coverage bytes over a rectangle: zero outside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Coverage {
    rect: DeviceRect,
    bytes: Vec<u8>,
}

impl Coverage {
    /// The coverage of the intersection of `regions` (none covers every
    /// pixel) in `window`.
    pub(crate) fn of(regions: &[Region], window: DeviceRect) -> Self {
        let areas = areas(regions, window);
        let rect = areas.rect();
        let mut bytes = Vec::with_capacity(pixels(rect));
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                bytes.push(quantize_unit(areas.get(x, y)));
            }
        }
        Self { rect, bytes }
    }

    /// The pixels that can be covered.
    pub(crate) fn rect(&self) -> DeviceRect {
        self.rect
    }

    /// The coverage of `(x, y)`.
    pub(crate) fn get(&self, x: u32, y: u32) -> u8 {
        index(self.rect, x, y)
            .and_then(|i| self.bytes.get(i))
            .copied()
            .unwrap_or(0)
    }

    /// The covered pixels and their coverage, row by row.
    pub(crate) fn pixels(&self) -> impl Iterator<Item = (u32, u32, u8)> + '_ {
        let rect = self.rect;
        (rect.y0..rect.y1)
            .flat_map(move |y| (rect.x0..rect.x1).map(move |x| (x, y)))
            .zip(&self.bytes)
            .filter(|&(_, &c)| c > 0)
            .map(|((x, y), &c)| (x, y, c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_raster::{LineCap, PathBuilder, Stroke};

    fn window(width: u32, height: u32) -> DeviceRect {
        DeviceRect {
            x0: 0,
            y0: 0,
            x1: width,
            y1: height,
        }
    }

    #[test]
    fn coverage_is_the_quantized_joint_area_of_mark_and_clips() {
        let limits = Limits::default();
        let mark = rect(
            Rect::from_corners(0.5, 0.0, 4.0, 4.0).unwrap(),
            Transform::IDENTITY,
            &limits,
        )
        .unwrap();
        let clip = rect(
            Rect::from_corners(0.0, 0.25, 4.0, 4.0).unwrap(),
            Transform::IDENTITY,
            &limits,
        )
        .unwrap();
        let coverage = Coverage::of(&[mark, clip], window(4, 4));
        // Pixel (0, 0): 1/2 × 3/4 of its area.
        assert_eq!(coverage.get(0, 0), quantize_unit(0.375));
        assert_eq!(coverage.get(1, 0), quantize_unit(0.75));
        assert_eq!(coverage.get(0, 1), 128);
        assert_eq!(coverage.get(2, 2), 255);
        assert_eq!(coverage.pixels().count(), 16);
    }

    #[test]
    fn strokes_cover_their_outline() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(1.0, 2.0));
        path.line_to(Point::new(7.0, 2.0)).unwrap();
        let pen = Pen {
            stroke: Stroke {
                width: 2.0,
                cap: LineCap::Butt,
                ..Stroke::default()
            },
            transform: Transform::IDENTITY,
            adjustment: Adjustment::None,
        };
        let region = stroke(
            &path.finish(),
            &pen,
            Transform::IDENTITY,
            &Limits::default(),
        )
        .unwrap();
        let coverage = Coverage::of(&[region], window(8, 4));
        assert_eq!(
            coverage.rect(),
            DeviceRect {
                x0: 1,
                y0: 1,
                x1: 7,
                y1: 3
            }
        );
        assert!(coverage.pixels().all(|(_, _, c)| c == 255));
        let singular = Pen {
            transform: Transform::scale(0.0, 1.0),
            ..pen
        };
        let mut path = PathBuilder::new();
        path.move_to(Point::new(1.0, 2.0));
        assert!(
            stroke(
                &path.finish(),
                &singular,
                Transform::IDENTITY,
                &Limits::default()
            )
            .is_err()
        );
    }
}
