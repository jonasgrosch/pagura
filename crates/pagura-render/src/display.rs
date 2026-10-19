//! Immutable PDF display lists and tile composition (ADR 0004).

use crate::color::Color;
use crate::{RenderOptions, Tile};
use pagura_core::{Error, Limits, Rect, Result};
use pagura_document::{Page, Rotation};
use pagura_raster::{FillRule, Path, Pixmap, Stroke, Transform};
use std::sync::{Arc, OnceLock};

mod glyphs;
mod opaque;
pub(crate) mod scene;
pub(crate) use glyphs::{GlyphCache, GlyphRun, Placement};

#[derive(Debug, Clone)]
pub(crate) struct Clip {
    pub(crate) parent: Option<Arc<Self>>,
    pub(crate) path: Arc<Path>,
    pub(crate) rule: FillRule,
}

#[derive(Debug)]
pub(crate) enum Mark {
    Fill(FillRule),
    /// A stroke with its pen, the CTM the pen is in (8.5.3.1) and whether
    /// stroke adjustment applies (10.7.5).
    Stroke {
        pen: Stroke,
        ctm: Transform,
        adjust: bool,
    },
}

#[derive(Debug)]
pub(crate) enum Command {
    Path(PathCommand),
    Glyphs(Box<GlyphRun>),
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
/// Compile with [`crate::compile_page`] or [`crate::RenderContext`].
#[derive(Debug)]
pub struct DisplayList {
    pub(crate) commands: Vec<Command>,
    pub(crate) crop: Rect,
    pub(crate) rotation: Rotation,
    pub(crate) user_unit: f64,
    pub(crate) limits: Limits,
    pub(crate) group: crate::transparency::Attributes,
    pub(crate) print_intent: bool,
    pub(crate) separations: OnceLock<Result<Arc<crate::compositor::Separations>>>,
    pub(crate) scene: OnceLock<scene::Scene>,
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
            print_intent: false,
            separations: OnceLock::new(),
            scene: OnceLock::new(),
        }
    }

    /// What the page asks of composition, classified once.
    pub(crate) fn scene(&self) -> scene::Scene {
        *self
            .scene
            .get_or_init(|| scene::Scene::of(&self.commands, &self.group))
    }

    pub(crate) fn separations(&self) -> Result<Arc<crate::compositor::Separations>> {
        self.separations
            .get_or_init(|| {
                crate::compositor::Separations::new(&self.commands, self.limits).map(Arc::new)
            })
            .clone()
    }

    pub(crate) fn preview_separations(
        &self,
        options: &RenderOptions,
    ) -> Result<Arc<crate::compositor::Separations>> {
        if options.overprint.resolve(self.print_intent) == crate::overprint::Policy::Simulate {
            self.separations()
        } else {
            Ok(Arc::default())
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

    /// Conservative bounds of painted content in default user space, clamped to
    /// the crop box (12.3.2.2). Returns `None` for empty or disjoint culling geometry.
    ///
    /// Uses the renderer's culling geometry: cubic control hulls, stroke
    /// envelopes, group bounds and hard-clip envelopes. These can include empty
    /// space; the result is not a tight geometric intersection. Unbounded
    /// shadings use the crop. Point contours and hairlines use a one-unit
    /// reference device; annotation view flags introduce no zoom or rotation.
    ///
    /// # Errors
    /// Returns `Limit` for overflowing geometry or exhausted traversal budgets.
    pub fn content_bounds(&self) -> Result<Option<Rect>> {
        crate::compositor::content_bounds(self)
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
        // Knockout groups stay off the 8-bit path, though opaque ones reduce
        // to source-over: nearly every page has one (ADR 0004, known defect).
        if self.preview_separations(options)?.enabled || !self.scene().opaque {
            return crate::compositor::render(self, options, tile);
        }
        self.render_opaque(options, tile)
    }

    pub(crate) fn device_transform(&self, options: &RenderOptions) -> Transform {
        let r = self.crop;
        let rotation = match self.rotation {
            Rotation::None => Transform::new(1.0, 0.0, 0.0, -1.0, -r.x0(), r.y1()),
            Rotation::Clockwise90 => Transform::new(0.0, 1.0, 1.0, 0.0, -r.y0(), -r.x0()),
            Rotation::Clockwise180 => Transform::new(-1.0, 0.0, 0.0, 1.0, r.x1(), -r.y0()),
            Rotation::Clockwise270 => Transform::new(0.0, -1.0, -1.0, 0.0, r.y1(), r.x1()),
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
            Command::Glyphs(run) => run.glyphs.len(),
        })
        .sum()
}

pub(crate) fn contextual(error: Error, position: pagura_core::Position) -> Error {
    error.at(position).in_object_opt(position.object())
}
impl Command {
    pub(crate) fn position(&self) -> pagura_core::Position {
        match self {
            Self::Path(p) => p.position,
            Self::Glyphs(run) => run.paint.position,
            Self::Image(i) => i.position,
            Self::Shading(s) => s.position,
            Self::Group(g) => g.position,
        }
    }
}

#[cfg(test)]
mod tests;
