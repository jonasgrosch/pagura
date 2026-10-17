//! Immutable PDF display lists and tile composition (ADR 0004).

use crate::color::Color;
use crate::{RenderOptions, Tile};
use pagura_core::{Error, Limits, Rect, Result};
use pagura_document::{Page, Rotation};
use pagura_raster::{
    FillRule, Mask, Paint, Path, PathBuilder, Pixmap, Stroke, Transform, fill_path_at,
    stroke_path_adjusted_at, stroke_path_at,
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
    Stroke(Stroke, Transform, bool),
}

#[derive(Debug)]
pub(crate) enum Command {
    Path(PathCommand),
    Image(ImageCommand),
    Group(crate::transparency::Group),
    Shading(ShadingCommand),
}

#[derive(Debug)]
pub(crate) struct ImageCommand {
    pub(crate) image: Arc<crate::interpreter::images::Image>,
    pub(crate) transform: Transform,
    pub(crate) clip: Option<Arc<Clip>>,
    pub(crate) stencil: Color,
    pub(crate) composite: crate::transparency::Composite,
    pub(crate) transfer: Option<crate::functions::Transfer>,
    pub(crate) position: pagura_core::Position,
}

#[derive(Debug)]
pub(crate) struct ShadingCommand {
    pub(crate) shading: Arc<crate::shading::Shading>,
    pub(crate) position: pagura_core::Position,
    pub(crate) transform: Transform,
    pub(crate) clip: Option<Arc<Clip>>,
    pub(crate) composite: crate::transparency::Composite,
    pub(crate) transfer: Option<crate::functions::Transfer>,
    pub(crate) background: bool,
}

#[derive(Debug)]
pub(crate) struct PathCommand {
    // Construction CTMs have already mapped every path point into default
    // user space. A stroke's pen CTM is retained independently (8.5.2).
    pub(crate) path: Arc<Path>,
    pub(crate) mark: Mark,
    pub(crate) clip: Option<Arc<Clip>>,
    pub(crate) color: Color,
    pub(crate) position: pagura_core::Position,
    pub(crate) transfer: Option<crate::functions::Transfer>,
    pub(crate) composite: crate::transparency::Composite,
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
    pub(crate) group: crate::transparency::Attributes,
}

impl DisplayList {
    pub(crate) fn new(page: &Page, limits: Limits) -> Self {
        Self {
            commands: Vec::new(),
            crop: page.crop_box(),
            rotation: page.rotation(),
            user_unit: page.user_unit(),
            limits,
            group: crate::transparency::Attributes::default(),
        }
    }

    /// Number of painted marks (a fill-and-stroke operation produces two).
    pub fn len(&self) -> usize {
        count_marks(&self.commands)
    }

    /// Whether the page contains no painted marks.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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
        if self.group.knockout
            || self
                .group
                .model
                .is_some_and(|m| m != crate::transparency::Model::Rgb)
            || !opaque_commands(&self.commands)
        {
            return crate::compositor::render(self, options, tile);
        }
        self.render_opaque(options, tile)
    }

    fn render_opaque(&self, options: &RenderOptions, tile: Tile) -> Result<Pixmap> {
        let device = self.device_transform(options);
        let mut target =
            Pixmap::new_with_limits(tile.width, tile.height, options.background, self.limits)?;
        // The crop is a geometric clip, including fractional final rows/columns.
        let mut crop = PathBuilder::new();
        crop.rect(
            self.crop.x0,
            self.crop.y0,
            self.crop.width(),
            self.crop.height(),
        );
        let crop = Mask::from_path_with_limits(
            tile.width,
            tile.height,
            &crop.finish(),
            device,
            FillRule::NonZero,
            self.limits,
            (tile.x, tile.y),
        )?;
        let mut last_clip = None;
        let mut mask = crop.clone();
        let mut stack = vec![self.commands.iter()];
        while let Some(iter) = stack.last_mut() {
            let Some(command) = iter.next() else {
                stack.pop();
                continue;
            };
            let command = match command {
                Command::Path(command) => command,
                Command::Group(group) => {
                    if stack.len() >= self.limits.depth {
                        return Err(Error::limit("group nesting exceeded"));
                    }
                    stack.push(group.commands.iter());
                    continue;
                }
                Command::Shading(_) | Command::Image(_) => {
                    return Err(Error::invalid_argument("shading in opaque renderer"));
                }
            };
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
                    let next = Mask::from_path_with_limits(
                        tile.width,
                        tile.height,
                        &node.path,
                        device,
                        node.rule,
                        self.limits,
                        (tile.x, tile.y),
                    )?;
                    mask.intersect(&next)?;
                }
                last_clip = identity;
            }
            command
                .rasterize(&mut target, device, &mask, (tile.x, tile.y))
                .map_err(|e| contextual(e, command.position))?;
        }
        Ok(target)
    }

    pub(crate) fn device_transform(&self, options: &RenderOptions) -> Transform {
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

fn count_marks(commands: &[Command]) -> usize {
    commands
        .iter()
        .map(|command| match command {
            Command::Path(_) | Command::Shading(_) | Command::Image(_) => 1,
            Command::Group(group) => count_marks(&group.commands),
        })
        .sum()
}

impl PathCommand {
    fn rasterize(
        &self,
        target: &mut Pixmap,
        device: Transform,
        mask: &Mask,
        origin: (u32, u32),
    ) -> Result<()> {
        let paint = Paint::Solid(self.color.to_rgba(self.transfer.as_ref())?);
        match &self.mark {
            Mark::Fill(rule) => fill_path_at(
                target,
                &self.path,
                device,
                *rule,
                &paint,
                Some(mask),
                origin,
            )?,
            Mark::Stroke(stroke, ctm, adjust) => {
                let inverse = ctm.inverse().ok_or(Error::unsupported(
                    crate::features::SINGULAR_STROKE,
                    "singular stroke transform is not implemented",
                ))?;
                let path = self.path.transformed(inverse);
                let rasterize = if *adjust {
                    stroke_path_adjusted_at
                } else {
                    stroke_path_at
                };
                rasterize(
                    target,
                    &path,
                    ctm.then(device),
                    stroke,
                    &paint,
                    Some(mask),
                    origin,
                )?;
            }
        }
        Ok(())
    }
}

/// Opaque Normal non-knockout RGB groups are source-over associative
/// (11.3.3, 11.4.5); no intermediate surface or colour conversion is needed.
fn opaque_commands(commands: &[Command]) -> bool {
    commands.iter().all(|command| match command {
        Command::Path(p) => p.composite.simple() && matches!(p.color, Color::Solid { .. }),
        Command::Group(g) => {
            g.composite.simple()
                && g.attributes.view.is_none()
                && !g.attributes.knockout
                && (!g.attributes.isolated
                    || g.attributes
                        .model
                        .is_none_or(|m| m == crate::transparency::Model::Rgb))
                && opaque_commands(&g.commands)
        }
        Command::Shading(_) | Command::Image(_) => false,
    })
}

pub(crate) fn contextual(error: Error, position: pagura_core::Position) -> Error {
    let error = error.at(position);
    if let pagura_core::Position::Stream { object, .. } = position {
        error.in_object(object)
    } else {
        error
    }
}
impl Command {
    pub(crate) fn position(&self) -> pagura_core::Position {
        match self {
            Self::Path(p) => p.position,
            Self::Image(i) => i.position,
            Self::Shading(s) => s.position,
            Self::Group(g) => g.position,
        }
    }
}
