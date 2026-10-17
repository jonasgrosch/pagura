//! Immutable PDF display lists and tile composition (ADR 0004).

use crate::color::Color;
use crate::{RenderOptions, Tile};
use pagura_core::{Error, Limits, Rect, Result};
use pagura_document::{Page, Rotation};
use pagura_raster::{
    FillRule, Mask, Paint, Path, PathBuilder, Pixmap, Stroke, Transform, fill_path_at,
    stroke_path_at,
};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct Clip {
    pub(crate) parent: Option<Arc<Self>>,
    pub(crate) path: Arc<Path>,
    pub(crate) rule: FillRule,
}

#[derive(Debug)]
pub(crate) enum Mark {
    Fill(FillRule),
    Stroke(Stroke, Transform),
}

#[derive(Debug)]
pub(crate) struct Command {
    // Construction CTMs have already mapped every path point into default
    // user space. A stroke's pen CTM is retained independently (8.5.2).
    pub(crate) path: Arc<Path>,
    pub(crate) mark: Mark,
    pub(crate) clip: Option<Arc<Clip>>,
    pub(crate) color: Color,
}

/// An immutable interpreted page, retaining source colours and default-user-space
/// geometry separately from device pixels (ISO 32000-2, 8.4–8.6; ADR 0004).
///
/// Compile with [`crate::compile_page`]; [`crate::render_page`] and
/// [`crate::render_tile`] compile and render in one call.
#[derive(Debug)]
pub struct DisplayList {
    pub(crate) commands: Vec<Command>,
    pub(crate) crop: Rect,
    pub(crate) rotation: Rotation,
    pub(crate) user_unit: f64,
    pub(crate) limits: Limits,
}

impl DisplayList {
    pub(crate) fn new(page: &Page, limits: Limits) -> Self {
        Self {
            commands: Vec::new(),
            crop: page.crop_box(),
            rotation: page.rotation(),
            user_unit: page.user_unit(),
            limits,
        }
    }

    /// Number of painted marks (a fill-and-stroke operation produces two).
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether the page contains no painted marks.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Device size after crop, rotation, user unit and scale (8.3.2).
    ///
    /// # Errors
    /// Returns `InvalidArgument` for invalid scales and `Limit` for overflow.
    pub fn device_size(&self, options: &RenderOptions) -> Result<(u32, u32)> {
        crate::device_extent(self.crop, self.rotation, self.user_unit, options)
    }

    /// Renders the complete display list into premultiplied RGBA8.
    ///
    /// # Errors
    /// Returns scale, raster geometry and resource-limit errors. No partial
    /// surface is returned on failure.
    pub fn render(&self, options: &RenderOptions) -> Result<Pixmap> {
        let (width, height) = self.device_size(options)?;
        self.render_tile(
            options,
            Tile {
                x: 0,
                y: 0,
                width,
                height,
            },
        )
    }

    /// Renders a device tile; any partition assembles byte-for-byte into
    /// [`Self::render`]. Integer tile origins select global device pixels without altering flattening.
    ///
    /// # Errors
    /// As [`Self::render`], plus `InvalidArgument` for tiles outside the page.
    pub fn render_tile(&self, options: &RenderOptions, tile: Tile) -> Result<Pixmap> {
        let (width, height) = self.device_size(options)?;
        if tile.width == 0
            || tile.height == 0
            || tile.x.checked_add(tile.width).is_none_or(|n| n > width)
            || tile.y.checked_add(tile.height).is_none_or(|n| n > height)
        {
            return Err(Error::invalid_argument("tile lies outside the page"));
        }
        let device = self.device_transform(options);
        let mut target = Pixmap::new(
            tile.width,
            tile.height,
            options.background,
            self.limits.pixels,
        )?;
        // The crop is a geometric clip, including fractional final rows/columns.
        let mut crop = PathBuilder::new();
        crop.rect(
            self.crop.x0,
            self.crop.y0,
            self.crop.width(),
            self.crop.height(),
        );
        let crop = Mask::from_path_at(
            tile.width,
            tile.height,
            &crop.finish(),
            device,
            FillRule::NonZero,
            self.limits.pixels,
            (tile.x, tile.y),
        )?;
        let mut last_clip = None;
        let mut mask = crop.clone();
        for command in &self.commands {
            let identity = command.clip.as_ref().map(Arc::as_ptr);
            if identity != last_clip {
                mask = crop.clone();
                let mut clip = command.clip.as_deref();
                let mut chain = Vec::new();
                while let Some(node) = clip {
                    chain.push(node);
                    clip = node.parent.as_deref();
                }
                for node in chain.into_iter().rev() {
                    let next = Mask::from_path_at(
                        tile.width,
                        tile.height,
                        &node.path,
                        device,
                        node.rule,
                        self.limits.pixels,
                        (tile.x, tile.y),
                    )?;
                    mask.intersect(&next)?;
                }
                last_clip = identity;
            }
            let paint = Paint::Solid(command.color.to_rgba());
            match &command.mark {
                Mark::Fill(rule) => fill_path_at(
                    &mut target,
                    &command.path,
                    device,
                    *rule,
                    &paint,
                    Some(&mask),
                    (tile.x, tile.y),
                )?,
                Mark::Stroke(stroke, ctm) => {
                    let inverse = ctm.inverse().ok_or(Error::unsupported(
                        crate::features::SINGULAR_STROKE,
                        "singular stroke transform is not implemented",
                    ))?;
                    let path = command.path.transformed(inverse);
                    stroke_path_at(
                        &mut target,
                        &path,
                        ctm.then(device),
                        stroke,
                        &paint,
                        Some(&mask),
                        (tile.x, tile.y),
                    )?;
                }
            }
        }
        Ok(target)
    }

    fn device_transform(&self, options: &RenderOptions) -> Transform {
        let r = self.crop;
        let rotation = match self.rotation {
            Rotation::None => Transform::new(1.0, 0.0, 0.0, -1.0, -r.x0, r.y1),
            Rotation::Clockwise90 => Transform::new(0.0, 1.0, 1.0, 0.0, -r.y0, -r.x0),
            Rotation::Clockwise180 => Transform::new(-1.0, 0.0, 0.0, 1.0, r.x1, -r.y0),
            Rotation::Clockwise270 => Transform::new(0.0, -1.0, -1.0, 0.0, r.y1, r.x1),
        };
        rotation.then(Transform::scale(
            options.scale * self.user_unit,
            options.scale * self.user_unit,
        ))
    }
}
